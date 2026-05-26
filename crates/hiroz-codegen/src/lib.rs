// Standalone codegen modules (Phase 1+)
pub mod discovery;
pub mod generator;
pub mod hashing;
pub mod parser;
pub mod resolver;
pub mod types;

// Legacy adapters (will be migrated to use ResolvedMessage)
#[cfg(feature = "protobuf")]
pub mod protobuf_generator;

pub mod python_msgspec_generator;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
// Re-exports for backward compatibility
pub use types::{ResolvedMessage, ResolvedService};

/// Configuration for the message generator
pub struct GeneratorConfig {
    /// Generate CDR-compatible serde types
    pub generate_cdr: bool,

    /// Generate protobuf definitions
    pub generate_protobuf: bool,

    /// Generate MessageTypeInfo trait impls
    pub generate_type_info: bool,

    /// Humble compatibility mode (no ServiceEventInfo, placeholder type hashes)
    pub is_humble: bool,

    pub output_dir: PathBuf,

    /// External crate path for standard message types (e.g., "hiroz_msgs").
    /// When set, references to packages NOT in the local package set will use
    /// fully qualified paths: `::{external_crate}::ros::{package}::{Type}`
    pub external_crate: Option<String>,

    /// Set of local package names (used with external_crate to determine
    /// which types need external references)
    pub local_packages: std::collections::HashSet<String>,

    /// Output JSON definitions for external generators (Go, Python, etc.)
    pub json_out: Option<PathBuf>,

    /// Packages to skip during protobuf generation (CDR generation is unaffected).
    /// Useful when a package's message types are intentionally not exposed via protobuf.
    pub protobuf_excluded_packages: std::collections::HashSet<String>,
}

/// Message generator that orchestrates parsing, resolution, and code generation
pub struct MessageGenerator {
    config: GeneratorConfig,
}

impl MessageGenerator {
    pub fn new(config: GeneratorConfig) -> Self {
        Self { config }
    }

    /// Primary generation method - uses pure Rust codegen pipeline.
    ///
    /// This is a thin wrapper around [`generate_from_msg_files_with_deps`] with no
    /// dependency-only packages.
    ///
    /// [`generate_from_msg_files_with_deps`]: Self::generate_from_msg_files_with_deps
    pub fn generate_from_msg_files(&self, packages: &[&Path]) -> Result<()> {
        self.generate_from_msg_files_with_deps(packages, &[])
    }

    /// Generation method that takes additional dependency-only packages.
    ///
    /// `packages` are parsed, resolved AND emitted as Rust code. `dep_packages` are
    /// parsed and resolved (so their type descriptions are available for hash
    /// computation of types in `packages` that reference them) but no Rust code is
    /// emitted for them — these typically come from another crate (e.g. `hiroz_msgs`).
    ///
    /// This is the mechanism that lets [`generate_user_messages`] compute correct
    /// RIHS01 hashes for user messages that reference bundled types like
    /// `std_msgs/Header` without the user having to manually add bundled paths to
    /// `HIROZ_MSG_PATH`.
    pub fn generate_from_msg_files_with_deps(
        &self,
        packages: &[&Path],
        dep_packages: &[&Path],
    ) -> Result<()> {
        // Discover and parse all messages, services, and actions for the user
        // packages.
        let (user_messages, user_services, user_actions) = discovery::discover_all(packages)
            .context("Failed to discover messages, services, and actions")?;

        let user_messages = Self::filter_messages(user_messages);
        let user_services = Self::filter_services(user_services);

        // Track which packages belong to the "user" (emit-code) set so we can
        // filter the resolver's combined output later.
        let user_package_names: std::collections::HashSet<String> = user_messages
            .iter()
            .map(|m| m.package.clone())
            .chain(user_services.iter().map(|s| s.package.clone()))
            .chain(user_actions.iter().map(|a| a.package.clone()))
            .collect();

        println!(
            "cargo:info=Discovered {} user messages, {} user services, and {} user actions",
            user_messages.len(),
            user_services.len(),
            user_actions.len()
        );

        // Discover dependency-only packages (parsed for hash resolution but not emitted).
        let (dep_messages, dep_services, dep_actions) = discovery::discover_all(dep_packages)
            .context("Failed to discover dependency messages, services, and actions")?;
        let dep_messages = Self::filter_messages(dep_messages);
        let dep_services = Self::filter_services(dep_services);

        if !dep_packages.is_empty() {
            println!(
                "cargo:info=Discovered {} dependency messages, {} dependency services, and {} dependency actions",
                dep_messages.len(),
                dep_services.len(),
                dep_actions.len()
            );
        }

        // Track which packages have been loaded as actual TypeDescriptions — these
        // do NOT need the resolver's "external_packages" shortcut.
        let mut loaded_packages: std::collections::HashSet<String> =
            self.config.local_packages.clone();
        for m in &dep_messages {
            loaded_packages.insert(m.package.clone());
        }
        for s in &dep_services {
            loaded_packages.insert(s.package.clone());
        }
        for a in &dep_actions {
            loaded_packages.insert(a.package.clone());
        }

        // Resolve dependencies and calculate type hashes.
        // The resolver's "external_packages" set is a fallback that lets references
        // to types we don't actually have descriptions for be treated as resolved
        // (with a wrong hash). With this set populated we never trigger that path
        // for any package whose definitions we've actually loaded.
        let external_packages = if self.config.external_crate.is_some() {
            // Standard ROS 2 packages that are provided by `hiroz_msgs`.
            let standard_packages: std::collections::HashSet<String> = [
                "builtin_interfaces",
                "std_msgs",
                "geometry_msgs",
                "sensor_msgs",
                "nav_msgs",
                "action_msgs",
                "unique_identifier_msgs",
                "service_msgs",
                "example_interfaces",
                "action_tutorials_interfaces",
                "test_msgs",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();

            standard_packages
                .difference(&loaded_packages)
                .cloned()
                .collect()
        } else {
            std::collections::HashSet::new()
        };

        let mut resolver =
            resolver::Resolver::with_external_packages(self.config.is_humble, external_packages);

        // Resolve dependency messages first so their type descriptions are
        // registered before any user message that references them.
        if !dep_messages.is_empty() {
            resolver
                .resolve_messages(dep_messages)
                .context("Failed to resolve dependency message hashes")?;
        }
        if !dep_services.is_empty() {
            resolver
                .resolve_services(dep_services)
                .context("Failed to resolve dependency service hashes")?;
        }
        if !dep_actions.is_empty() {
            resolver
                .resolve_actions(dep_actions)
                .context("Failed to resolve dependency action hashes")?;
        }

        // Now resolve the user messages — their hashes will be computed using the
        // dependency type descriptions registered above.
        //
        // `Resolver::resolve_messages` returns *all* messages currently registered
        // in the resolver (including the dependency messages resolved above), so we
        // filter the returned vector to keep only types in the user package set —
        // those are the ones we want to emit Rust code for.
        let all_resolved = resolver
            .resolve_messages(user_messages)
            .context("Failed to resolve message dependencies")?;
        let resolved_messages: Vec<_> = all_resolved
            .into_iter()
            .filter(|m| user_package_names.contains(&m.parsed.package))
            .collect();
        let resolved_services = resolver
            .resolve_services(user_services)
            .context("Failed to resolve service dependencies")?;
        let resolved_actions = resolver
            .resolve_actions(user_actions)
            .context("Failed to resolve action dependencies")?;

        println!(
            "cargo:info=Resolved {} user messages, {} user services, and {} user actions for codegen",
            resolved_messages.len(),
            resolved_services.len(),
            resolved_actions.len()
        );

        // Export JSON definitions for external generators
        if let Some(json_path) = &self.config.json_out {
            generator::json::export_json(
                &resolved_messages,
                &resolved_services,
                &resolved_actions,
                json_path,
            )?;
            println!("cargo:info=Exported JSON manifest to {:?}", json_path);
        }

        // Generate CDR-compatible types (using pure Rust codegen with ZBuf support).
        // Only user messages are emitted — dependency packages are assumed to be
        // generated by another crate (e.g. `hiroz_msgs`).
        if self.config.generate_cdr {
            self.generate_cdr_types(&resolved_messages, &resolved_services, &resolved_actions)?;
        }

        // Generate protobuf types
        #[cfg(feature = "protobuf")]
        if self.config.generate_protobuf {
            self.generate_protobuf_types(&resolved_messages)?;
        }

        Ok(())
    }

    /// Filter out problematic messages (actionlib, wstring, etc.)
    fn filter_messages(
        messages: Vec<crate::types::ParsedMessage>,
    ) -> Vec<crate::types::ParsedMessage> {
        let filtered: Vec<_> = messages
            .into_iter()
            .filter(|msg| {
                let full_name = format!("{}/{}", msg.package, msg.name);

                // Filter out old ROS 1 actionlib_msgs (deprecated)
                // Note: ROS 2 action messages (Goal/Result/Feedback) are now generated from .action files
                if full_name.starts_with("actionlib_msgs/") {
                    println!(
                        "cargo:info=Filtered deprecated actionlib_msgs: {}",
                        full_name
                    );
                    return false;
                }

                // Filter out redundant service Request/Response message files
                // ROS 2 Humble ships with *_Request.msg and *_Response.msg that duplicate
                // the messages auto-generated from .srv files
                if msg.name.ends_with("_Request") || msg.name.ends_with("_Response") {
                    println!("cargo:info=Filtered service msg file: {}", full_name);
                    return false;
                }

                // Also filter any message file in the "srv" directory (sometimes ROS puts srv msgs there)
                if msg.path.to_string_lossy().contains("/srv/") {
                    println!("cargo:info=Filtered srv directory message: {}", full_name);
                    return false;
                }

                // Filter out messages with wstring fields
                let has_wstring = msg
                    .fields
                    .iter()
                    .any(|field| field.field_type.base_type.contains("wstring"));

                if has_wstring {
                    println!(
                        "cargo:warning=Skipping message {} due to wstring field (unsupported)",
                        full_name
                    );
                    return false;
                }

                true
            })
            .collect();

        println!(
            "cargo:info=After filtering: {} messages remain",
            filtered.len()
        );
        filtered
    }

    /// Filter out problematic services
    fn filter_services(
        services: Vec<crate::types::ParsedService>,
    ) -> Vec<crate::types::ParsedService> {
        services
            .into_iter()
            .filter(|srv| {
                let full_name = format!("{}/{}", srv.package, srv.name);
                !full_name.starts_with("actionlib_msgs/")
            })
            .collect()
    }

    /// Generate CDR-compatible Rust types with ZBuf support
    fn generate_cdr_types(
        &self,
        messages: &[ResolvedMessage],
        services: &[ResolvedService],
        actions: &[crate::types::ResolvedAction],
    ) -> Result<()> {
        use std::collections::BTreeMap;

        use quote::quote;

        // Group messages, services, and actions by package
        let mut packages: BTreeMap<String, Vec<&ResolvedMessage>> = BTreeMap::new();
        let mut package_services: BTreeMap<String, Vec<&ResolvedService>> = BTreeMap::new();
        let mut package_actions: BTreeMap<String, Vec<&crate::types::ResolvedAction>> =
            BTreeMap::new();

        for msg in messages {
            packages
                .entry(msg.parsed.package.clone())
                .or_default()
                .push(msg);
        }

        // Add service request/response messages and track services
        for srv in services {
            packages
                .entry(srv.parsed.package.clone())
                .or_default()
                .push(&srv.request);
            packages
                .entry(srv.parsed.package.clone())
                .or_default()
                .push(&srv.response);

            package_services
                .entry(srv.parsed.package.clone())
                .or_default()
                .push(srv);
        }

        // Add action goal/result/feedback messages and track actions
        for action in actions {
            packages
                .entry(action.parsed.package.clone())
                .or_default()
                .push(&action.goal);
            if let Some(ref result) = action.result {
                packages
                    .entry(action.parsed.package.clone())
                    .or_default()
                    .push(result);
            }
            if let Some(ref feedback) = action.feedback {
                packages
                    .entry(action.parsed.package.clone())
                    .or_default()
                    .push(feedback);
            }

            package_actions
                .entry(action.parsed.package.clone())
                .or_default()
                .push(action);
        }

        // Generate code for each package
        let mut all_tokens = proc_macro2::TokenStream::new();

        // Collect all package names
        let mut all_package_names = std::collections::BTreeSet::new();
        all_package_names.extend(packages.keys().cloned());
        all_package_names.extend(package_services.keys().cloned());
        all_package_names.extend(package_actions.keys().cloned());

        // Create generation context for external type references
        let gen_ctx = generator::rust::GenerationContext::new(
            self.config.external_crate.clone(),
            self.config.local_packages.clone(),
        );

        // Compute plain types across all messages once (bottom-up over full type graph)
        let all_msgs_vec: Vec<ResolvedMessage> =
            packages.values().flatten().map(|m| (*m).clone()).collect();
        let plain_types = generator::rust::compute_plain_types(&all_msgs_vec);

        for package_name in all_package_names {
            let package_ident = quote::format_ident!("{}", &package_name);

            // Generate message implementations
            let message_impls: Vec<_> = packages
                .get(&package_name)
                .map(|msgs| {
                    msgs.iter()
                        .map(|msg| {
                            generator::rust::generate_message_impl_with_cdr(
                                msg,
                                &gen_ctx,
                                &plain_types,
                            )
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()
                .context("Failed to generate message implementations")?
                .unwrap_or_default();

            // Generate service implementations
            let service_impls: Vec<_> = package_services
                .get(&package_name)
                .map(|srvs| {
                    srvs.iter()
                        .map(|srv| generator::rust::generate_service_impl(srv))
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()
                .context("Failed to generate service implementations")?
                .unwrap_or_default();

            // Generate action implementations
            let action_impls: Vec<_> = package_actions
                .get(&package_name)
                .map(|acts| {
                    acts.iter()
                        .map(|action| generator::rust::generate_action_impl(action))
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()
                .context("Failed to generate action implementations")?
                .unwrap_or_default();

            // Create submodules for services and actions
            let service_module = if !service_impls.is_empty() {
                quote! {
                    pub mod srv {
                        #(#service_impls)*
                    }
                }
            } else {
                quote! {}
            };

            let action_module = if !action_impls.is_empty() {
                quote! {
                    pub mod action {
                        #(#action_impls)*
                    }
                }
            } else {
                quote! {}
            };

            let pkg_tokens = quote! {
                pub mod #package_ident {
                    #(#message_impls)*
                    #service_module
                    #action_module
                }
            };

            all_tokens.extend(pkg_tokens);
        }

        // Wrap in ros module for namespacing
        let wrapped_tokens = quote! {
            #[allow(clippy::approx_constant, clippy::manual_is_multiple_of, clippy::let_and_return)]
            pub mod ros {
                #all_tokens
            }
        };

        // Format and write
        let syntax_tree: syn::File =
            syn::parse2(wrapped_tokens).context("Failed to parse generated code")?;
        let formatted_code = prettyplease::unparse(&syntax_tree);

        let output_file = self.config.output_dir.join("generated.rs");
        std::fs::write(&output_file, formatted_code)
            .with_context(|| format!("Failed to write generated code to {:?}", output_file))?;

        println!(
            "cargo:info=Generated {} CDR types with ZBuf support",
            messages.len() + services.len() + actions.len()
        );

        Ok(())
    }

    /// Generate protobuf types
    #[cfg(feature = "protobuf")]
    fn generate_protobuf_types(&self, messages: &[ResolvedMessage]) -> Result<()> {
        use crate::protobuf_generator::ProtobufMessageGenerator;

        let filtered: Vec<ResolvedMessage> = messages
            .iter()
            .filter(|m| {
                !self
                    .config
                    .protobuf_excluded_packages
                    .contains(&m.parsed.package)
            })
            .cloned()
            .collect();
        let messages = filtered.as_slice();

        let proto_dir = self.config.output_dir.join("proto");
        let generator = ProtobufMessageGenerator::new(&proto_dir);

        // Generate .proto files
        let proto_files = generator.generate_proto_files(messages)?;
        println!("cargo:info=Generated {} .proto files", proto_files.len());

        // Generate Rust code from .proto files
        let proto_output = self.config.output_dir.join("generated_proto.rs");
        generator.generate_rust_from_proto(&proto_files, &proto_output)?;

        // Generate MessageTypeInfo implementations
        let type_info_impls = generator.generate_type_info_impls(messages)?;
        let type_info_output = self.config.output_dir.join("protobuf_type_info.rs");
        std::fs::write(&type_info_output, type_info_impls)?;

        println!("cargo:info=Protobuf generation complete");
        Ok(())
    }
}

/// Return the path to the bundled message assets directory for the given distro.
///
/// `hiroz-codegen` ships its own copies of the standard ROS 2 interface packages
/// (`std_msgs`, `geometry_msgs`, `builtin_interfaces`, ...) under `assets/{distro}`.
/// These are included in the published crate (see the `include` field in
/// `Cargo.toml`), so this path resolves correctly whether `hiroz-codegen` is
/// consumed via a path/git dependency or from `crates.io`.
pub fn bundled_assets_dir(is_humble: bool) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let distro = if is_humble { "humble" } else { "jazzy" };
    manifest_dir.join("assets").join(distro)
}

/// Discover all bundled standard-package message paths shipped with `hiroz-codegen`.
///
/// Returns one path per package directory (e.g. `.../assets/jazzy/std_msgs`) for
/// every subdirectory of the bundled assets that contains `msg/`, `srv/`, or
/// `action/`. Returns an empty vector if the bundled assets directory does not
/// exist for some reason.
pub fn discover_bundled_packages(is_humble: bool) -> Result<Vec<PathBuf>> {
    let assets_dir = bundled_assets_dir(is_humble);
    if !assets_dir.exists() {
        println!(
            "cargo:warning=Bundled assets directory does not exist: {:?}",
            assets_dir
        );
        return Ok(Vec::new());
    }

    let mut packages = Vec::new();
    for entry in std::fs::read_dir(&assets_dir)
        .with_context(|| format!("Failed to read bundled assets dir {:?}", assets_dir))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let has_messages =
            path.join("msg").exists() || path.join("srv").exists() || path.join("action").exists();

        if has_messages {
            packages.push(path);
        }
    }

    Ok(packages)
}

/// Discover user message packages from the HIROZ_MSG_PATH environment variable.
///
/// The environment variable should contain a colon-separated list of paths,
/// where each path is a ROS2 package directory containing msg/, srv/, or action/ subdirs.
///
/// # Example
/// ```bash
/// export HIROZ_MSG_PATH="/path/to/my_msgs:/path/to/other_msgs"
/// ```
pub fn discover_user_packages() -> Result<Vec<PathBuf>> {
    let msg_path =
        std::env::var("HIROZ_MSG_PATH").context("HIROZ_MSG_PATH environment variable not set")?;

    let mut packages = Vec::new();

    for path_str in msg_path.split(':') {
        let path = PathBuf::from(path_str.trim());
        if path_str.trim().is_empty() {
            continue;
        }

        if !path.exists() {
            println!(
                "cargo:warning=HIROZ_MSG_PATH entry does not exist: {:?}",
                path
            );
            continue;
        }

        // Check if this path has msg/, srv/, or action/ subdirectories
        let has_messages =
            path.join("msg").exists() || path.join("srv").exists() || path.join("action").exists();

        if has_messages {
            println!("cargo:info=Found user package at: {:?}", path);
            packages.push(path);
        } else {
            println!(
                "cargo:warning=Path {:?} has no msg/, srv/, or action/ directory",
                path
            );
        }
    }

    if packages.is_empty() {
        anyhow::bail!("No valid message packages found in HIROZ_MSG_PATH");
    }

    Ok(packages)
}

/// High-level API for user crates to generate messages from `HIROZ_MSG_PATH`.
///
/// This function:
/// 1. Discovers user packages from the `HIROZ_MSG_PATH` environment variable.
/// 2. Loads the bundled standard ROS 2 packages shipped with `hiroz-codegen`
///    (`std_msgs`, `geometry_msgs`, `builtin_interfaces`, ...) so that user
///    messages referencing them get correct RIHS01 type hashes — without users
///    having to manually add bundled paths to `HIROZ_MSG_PATH`.
/// 3. Generates Rust code only for the user packages, with references to standard
///    types resolved as `::hiroz_msgs::ros::{package}::{Type}`.
///
/// # Arguments
/// * `output_dir` - Directory where `generated.rs` will be written
/// * `is_humble` - Set to true for ROS 2 Humble compatibility mode
///
/// # Example
/// ```rust,ignore
/// // In build.rs
/// fn main() -> anyhow::Result<()> {
///     let out_dir = std::env::var("OUT_DIR")?;
///     hiroz_codegen::generate_user_messages(&out_dir.into(), false)?;
///     println!("cargo:rerun-if-env-changed=HIROZ_MSG_PATH");
///     Ok(())
/// }
/// ```
pub fn generate_user_messages(output_dir: &Path, is_humble: bool) -> Result<()> {
    let packages = discover_user_packages()?;

    // Collect local package names (used to determine which generated code paths
    // refer to local types vs. types from `hiroz_msgs`).
    let local_packages: std::collections::HashSet<String> = packages
        .iter()
        .filter_map(|p| discovery::discover_package_name(p).ok())
        .collect();

    println!(
        "cargo:info=Generating user messages for packages: {:?}",
        local_packages
    );

    // Discover bundled standard packages — passed to the resolver as
    // dependency-only inputs so user messages referencing `std_msgs/Header`,
    // `builtin_interfaces/Time`, etc. get correct RIHS01 hashes. We do NOT add
    // them to `local_packages` because we don't want to emit Rust code for them
    // (the user's crate depends on `hiroz_msgs` for that).
    let dep_packages = discover_bundled_packages(is_humble)?;

    // Filter out any bundled package whose name collides with a user package —
    // user definitions take precedence.
    let dep_packages: Vec<PathBuf> = dep_packages
        .into_iter()
        .filter(|p| {
            discovery::discover_package_name(p)
                .ok()
                .is_none_or(|name| !local_packages.contains(&name))
        })
        .collect();

    if !dep_packages.is_empty() {
        println!(
            "cargo:info=Loading {} bundled standard packages from {:?} for hash resolution",
            dep_packages.len(),
            bundled_assets_dir(is_humble)
        );
    }

    let config = GeneratorConfig {
        generate_cdr: true,
        generate_protobuf: false,
        generate_type_info: true,
        is_humble,
        output_dir: output_dir.to_path_buf(),
        external_crate: Some("hiroz_msgs".to_string()),
        local_packages,
        json_out: None,
        protobuf_excluded_packages: std::collections::HashSet::new(),
    };

    let generator = MessageGenerator::new(config);
    let package_refs: Vec<&Path> = packages.iter().map(|p| p.as_path()).collect();
    let dep_refs: Vec<&Path> = dep_packages.iter().map(|p| p.as_path()).collect();
    generator.generate_from_msg_files_with_deps(&package_refs, &dep_refs)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serial_test::serial;

    use super::*;

    // Helper to safely set/remove env vars in Rust 2024
    // SAFETY: Tests using these are marked #[serial] to prevent data races
    fn set_env(key: &str, value: &str) {
        unsafe { std::env::set_var(key, value) };
    }

    fn remove_env(key: &str) {
        unsafe { std::env::remove_var(key) };
    }

    /// Test that user-defined messages with external dependencies generate correct code
    #[test]
    #[serial]
    fn test_generate_user_messages_with_external_deps() {
        // Create a temp directory structure
        let temp_dir = tempfile::tempdir().unwrap();
        let pkg_dir = temp_dir.path().join("my_test_msgs");
        let msg_dir = pkg_dir.join("msg");
        fs::create_dir_all(&msg_dir).unwrap();

        // Create a message that references external types (geometry_msgs/Point)
        let msg_content = r#"
string robot_id
geometry_msgs/Point position
bool is_active
"#;
        fs::write(msg_dir.join("TestStatus.msg"), msg_content).unwrap();

        // Create output directory
        let out_dir = temp_dir.path().join("out");
        fs::create_dir_all(&out_dir).unwrap();

        // Set the environment variable
        set_env("HIROZ_MSG_PATH", pkg_dir.to_str().unwrap());

        // Generate messages
        let result = generate_user_messages(&out_dir, false);
        assert!(
            result.is_ok(),
            "generate_user_messages failed: {:?}",
            result
        );

        // Read the generated file
        let generated_path = out_dir.join("generated.rs");
        assert!(generated_path.exists(), "generated.rs was not created");

        let generated_code = fs::read_to_string(&generated_path).unwrap();

        // Verify external type reference uses fully qualified path
        assert!(
            generated_code.contains("::hiroz_msgs::ros::geometry_msgs::Point"),
            "Generated code should use fully qualified path for external types.\nGenerated:\n{}",
            generated_code
        );

        // Verify struct was generated
        assert!(
            generated_code.contains("pub struct TestStatus"),
            "Generated code should contain TestStatus struct.\nGenerated:\n{}",
            generated_code
        );

        // Verify it's in the correct module
        assert!(
            generated_code.contains("pub mod my_test_msgs"),
            "Generated code should have my_test_msgs module.\nGenerated:\n{}",
            generated_code
        );

        // Clean up env var
        remove_env("HIROZ_MSG_PATH");
    }

    /// Test that services with external dependencies generate correct code
    #[test]
    #[serial]
    fn test_generate_user_services_with_external_deps() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pkg_dir = temp_dir.path().join("my_test_srvs");
        let srv_dir = pkg_dir.join("srv");
        fs::create_dir_all(&srv_dir).unwrap();

        // Create a service that references external types
        let srv_content = r#"
geometry_msgs/Point target
float64 speed
---
bool success
"#;
        fs::write(srv_dir.join("MoveTo.srv"), srv_content).unwrap();

        let out_dir = temp_dir.path().join("out");
        fs::create_dir_all(&out_dir).unwrap();

        set_env("HIROZ_MSG_PATH", pkg_dir.to_str().unwrap());

        let result = generate_user_messages(&out_dir, false);
        assert!(
            result.is_ok(),
            "generate_user_messages failed: {:?}",
            result
        );

        let generated_code = fs::read_to_string(out_dir.join("generated.rs")).unwrap();

        // Verify service request has external type reference
        assert!(
            generated_code.contains("pub struct MoveToRequest"),
            "Generated code should contain MoveToRequest struct"
        );
        assert!(
            generated_code.contains("::hiroz_msgs::ros::geometry_msgs::Point"),
            "Service request should use fully qualified path for external types"
        );

        // Verify service module
        assert!(
            generated_code.contains("pub mod srv"),
            "Generated code should have srv submodule"
        );

        remove_env("HIROZ_MSG_PATH");
    }

    /// Test discover_user_packages with missing env var
    #[test]
    #[serial]
    fn test_discover_user_packages_missing_env() {
        remove_env("HIROZ_MSG_PATH");
        let result = discover_user_packages();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("HIROZ_MSG_PATH environment variable not set")
        );
    }

    /// Test discover_user_packages with invalid path
    #[test]
    #[serial]
    fn test_discover_user_packages_no_valid_packages() {
        let temp_dir = tempfile::tempdir().unwrap();
        // Create a directory without msg/srv/action subdirs
        let empty_pkg = temp_dir.path().join("empty_pkg");
        fs::create_dir_all(&empty_pkg).unwrap();

        set_env("HIROZ_MSG_PATH", empty_pkg.to_str().unwrap());

        let result = discover_user_packages();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No valid message packages found")
        );

        remove_env("HIROZ_MSG_PATH");
    }
}
