{
  pkgs,
  git-hooks,
  system,
  rustfmtNightly,
  rustToolchain ? pkgs.rust-bin.stable.latest.default,
  docTools ? [ ],
  mkdocsPkg ? pkgs.python3.withPackages (
    ps: with ps; [
      mkdocs
      mkdocs-material
      mkdocs-material-extensions
      pymdown-extensions
    ]
  ),
}:
let
  # yamllint configuration
  yamlFormat = pkgs.formats.yaml { };
  yamllintConfigData = {
    extends = "default";
    rules = {
      line-length = {
        max = 120;
      };
      document-start = "disable";
      truthy = "disable";
    };
  };
  yamllintConfig = yamlFormat.generate ".yamllint.yaml" yamllintConfigData;

  # markdownlint configuration
  markdownlintConfig = {
    # Line length
    "MD013" = false;
    # Multiple consecutive blank lines
    "MD012" = false;
    # Multiple top-level headings in same document
    "MD025" = false;
    # Inline HTML
    "MD033" = {
      allowed_elements = [
        "div"
        "h1"
        "p"
        "strong"
        "a"
        "sub"
        "iframe"
        "script"
        "img"
      ];
    };
    # First line in file should be a top-level heading
    "MD041" = false;
  };
in
git-hooks.lib.${system}.run {
  src = ./..;
  tools = {
    rustfmt = rustfmtNightly;
    cargo = rustToolchain;
  };
  hooks = {
    # Rust tooling
    rustfmt.enable = true;
    taplo.enable = true;

    # Python tooling
    ruff = {
      enable = true;
      description = "Run ruff linter";
      entry = "${pkgs.ruff}/bin/ruff check --fix";
      files = "\\.py$";
      excludes = [ "crates/hiroz-msgs/python/hiroz_msgs_py/types/.*\\.py$" ];
    };

    ruff-format = {
      enable = true;
      description = "Run ruff formatter";
      entry = "${pkgs.ruff}/bin/ruff format";
      files = "\\.py$";
      excludes = [ "crates/hiroz-msgs/python/hiroz_msgs_py/types/.*\\.py$" ];
    };

    mypy = {
      enable = true;
      description = "Run mypy type checker";
      entry = "${pkgs.mypy}/bin/mypy";
      files = "crates/hiroz-py/tests/.*\\.py$|crates/hiroz-py/examples/.*\\.py$";
      pass_filenames = true;
      args = [
        "--ignore-missing-imports"
      ];
    };

    # General tooling
    yamllint = {
      enable = true;
      settings.configPath = "${yamllintConfig}";
    };

    markdownlint = {
      enable = true;
      settings.configuration = markdownlintConfig;
    };

    nixfmt-rfc-style.enable = true;

    # Documentation build check
    mkdocs-build = {
      enable = false; # re-enable after running `nix develop` to rebuild hooks with correct mkdocsPkg
      name = "mkdocs-build";
      description = "Build MkDocs documentation";
      entry = toString (
        pkgs.writeShellScript "mkdocs-build" ''
          exec ${mkdocsPkg}/bin/mkdocs build --strict
        ''
      );
      files = "docs/.*\\.(md|html|css|js)$|mkdocs\\.yml$";
      pass_filenames = false;
    };
  };
}
