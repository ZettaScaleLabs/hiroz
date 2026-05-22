use cxx::{ExternType, type_id};

unsafe impl ExternType for crate::ros::rosidl_message_type_support_t {
    type Id = type_id!("rosidl_message_type_support_t");
    type Kind = cxx::kind::Opaque;
}
unsafe impl ExternType for crate::ros::rosidl_service_type_support_t {
    type Id = type_id!("rosidl_service_type_support_t");
    type Kind = cxx::kind::Opaque;
}
unsafe impl ExternType for crate::ros::rosidl_type_hash_t {
    type Id = type_id!("rosidl_type_hash_t");
    type Kind = cxx::kind::Trivial;
}

#[cxx::bridge]
mod ffi {

    unsafe extern "C++" {
        include!("serde_bridge.h");

        type c_void = crate::c_void;
        type rosidl_message_type_support_t = crate::ros::rosidl_message_type_support_t;
        type rosidl_service_type_support_t = crate::ros::rosidl_service_type_support_t;
        type rosidl_type_hash_t = crate::ros::rosidl_type_hash_t;

        #[namespace = "serde_bridge"]
        unsafe fn get_message_typesupport(
            ts: *const rosidl_message_type_support_t,
        ) -> *const rosidl_message_type_support_t;

        #[namespace = "serde_bridge"]
        unsafe fn serialize_message(
            ts: *const rosidl_message_type_support_t,
            ros_message: *const c_void,
            out: &mut Vec<u8>,
        ) -> bool;

        #[namespace = "serde_bridge"]
        unsafe fn deserialize_message(
            ts: *const rosidl_message_type_support_t,
            data: &Vec<u8>,
            ros_message: *mut c_void,
        ) -> bool;

        #[namespace = "serde_bridge"]
        unsafe fn get_serialized_size(
            ts: *const rosidl_message_type_support_t,
            ros_message: *const c_void,
        ) -> usize;

        #[namespace = "serde_bridge"]
        unsafe fn get_message_name(ts: *const rosidl_message_type_support_t) -> String;

        #[namespace = "serde_bridge"]
        unsafe fn get_message_namespace(ts: *const rosidl_message_type_support_t) -> String;

        #[namespace = "serde_bridge"]
        unsafe fn get_service_typesupport(
            ts: *const rosidl_service_type_support_t,
        ) -> *const rosidl_service_type_support_t;

        #[namespace = "serde_bridge"]
        unsafe fn get_request_type_support(
            ts: *const rosidl_service_type_support_t,
        ) -> *const rosidl_message_type_support_t;

        #[namespace = "serde_bridge"]
        unsafe fn get_response_type_support(
            ts: *const rosidl_service_type_support_t,
        ) -> *const rosidl_message_type_support_t;

        // Type hash support (returns null stub on Humble)
        #[namespace = "serde_bridge"]
        unsafe fn get_service_type_hash(
            ts: *const rosidl_service_type_support_t,
        ) -> *const rosidl_type_hash_t;
    }
}

use ffi::*;
use hiroz::entity::{TypeHash, TypeInfo};

use crate::ros::rosidl_type_hash_t;

impl std::fmt::Display for rosidl_type_hash_t {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const HASH_PREFIX: &str = "RIHS01_";
        let hex_str: String = self.value.iter().map(|b| format!("{:02x}", b)).collect();
        write!(f, "{HASH_PREFIX}{hex_str}")
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MessageTypeSupport {
    ptr: *const rosidl_message_type_support_t,
}

impl AsRef<rosidl_message_type_support_t> for MessageTypeSupport {
    fn as_ref(&self) -> &rosidl_message_type_support_t {
        unsafe {
            assert!(!self.ptr.is_null());
            &*(*self).ptr
        }
    }
}

impl MessageTypeSupport {
    pub unsafe fn new(type_support: *const rosidl_message_type_support_t) -> zenoh::Result<Self> {
        let type_support = unsafe {
            let ts = get_message_typesupport(type_support);
            if ts.is_null() {
                tracing::error!("Failed to create the type support.");
                return Err(zenoh::Error::from(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Failed to get message type support - type support library not found or invalid",
                )));
            }
            ts
        };
        Ok(Self { ptr: type_support })
    }

    pub fn get_type_hash(&self) -> TypeHash {
        let hash = unsafe {
            let type_hash = self
                .as_ref()
                .get_type_hash_func
                .expect("Failed to get_type_hash_func")(self.as_ref());
            assert!(!type_hash.is_null());
            *type_hash
        };
        TypeHash::new(hash.version, hash.value)
    }

    pub unsafe fn serialize_message(&self, ros_message: *const c_void) -> Vec<u8> {
        // Get the serialized size (includes 4-byte CDR encapsulation header)
        let size = unsafe { self.get_serialized_size(ros_message) };
        // Add extra buffer for safety (alignment, padding, string overhead)
        let buffer_size = size + 256;
        let mut out = vec![0u8; buffer_size];
        let res = unsafe { serialize_message(self.as_ref(), ros_message, &mut out) };
        if !res {
            tracing::warn!("Failed to run serialize_message");
            vec![]
        } else {
            // Truncate to actual serialized size
            out.truncate(size);
            out
        }
    }

    pub unsafe fn get_serialized_size(&self, ros_message: *const c_void) -> usize {
        unsafe { get_serialized_size(self.as_ref(), ros_message) }
    }

    pub unsafe fn deserialize_message(&self, data: &Vec<u8>, ros_message: *mut c_void) -> bool {
        let res = unsafe { deserialize_message(self.as_ref(), data, ros_message) };
        if !res {
            tracing::warn!("Failed to run deserialize_message");
        }
        res
    }

    pub fn get_type_prefix(&self) -> String {
        let (name, namespace) = unsafe {
            let ts = self.as_ref();
            (get_message_name(ts), get_message_namespace(ts))
        };

        let ns = if namespace.is_empty() {
            ""
        } else {
            &(namespace + "::")
        };
        format!("{ns}dds_::{name}_")
    }

    /// Get the ROS type name in the format "namespace/msg/Name" (without DDS mangling)
    pub fn get_ros_type_name(&self) -> String {
        let (name, namespace) = unsafe {
            let ts = self.as_ref();
            (get_message_name(ts), get_message_namespace(ts))
        };

        // Remove trailing underscore from name (DDS mangling)
        let clean_name = name.strip_suffix('_').unwrap_or(&name);

        // Format as ROS type name: replace :: with / in namespace
        if namespace.is_empty() {
            clean_name.to_string()
        } else {
            // Replace C++ namespace separators (::) with ROS separators (/)
            let ros_namespace = namespace.replace("::", "/");
            format!("{}/{}", ros_namespace, clean_name)
        }
    }

    pub fn get_type_info(&self) -> TypeInfo {
        TypeInfo::new(&self.get_ros_type_name(), self.get_type_hash())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ServiceTypeSupport {
    ptr: *const rosidl_service_type_support_t,
    pub request: MessageTypeSupport,
    pub response: MessageTypeSupport,
}

impl AsRef<rosidl_service_type_support_t> for ServiceTypeSupport {
    fn as_ref(&self) -> &rosidl_service_type_support_t {
        unsafe {
            assert!(!self.ptr.is_null());
            &*(*self).ptr
        }
    }
}

impl ServiceTypeSupport {
    pub unsafe fn new(type_support: *const rosidl_service_type_support_t) -> zenoh::Result<Self> {
        let type_support = unsafe {
            let ts = get_service_typesupport(type_support);
            if ts.is_null() {
                // Clear any error state set by the C++ typesupport dispatch code
                crate::ros::rcutils_reset_error();
                tracing::error!("Failed to create the type support.");
                return Err(zenoh::Error::from(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Failed to get service type support - type support library not found or invalid",
                )));
            }
            ts
        };
        let (request, response) = unsafe {
            (
                get_request_type_support(type_support),
                get_response_type_support(type_support),
            )
        };
        Ok(Self {
            ptr: type_support,
            request: unsafe { MessageTypeSupport::new(request)? },
            response: unsafe { MessageTypeSupport::new(response)? },
        })
    }

    pub fn get_type_hash(&self) -> TypeHash {
        let hash = unsafe {
            let type_hash = get_service_type_hash(self.ptr);
            if type_hash.is_null() {
                // Fallback to response type's hash if service hash is not available
                return self.response.get_type_hash();
            }
            *type_hash
        };
        TypeHash::new(hash.version, hash.value)
    }

    pub fn get_type_info(&self) -> TypeInfo {
        let name_with_suffix = self.response.get_ros_type_name();
        let name = name_with_suffix
            .strip_suffix("_Response")
            .expect("Invalid Response type - must end with _Response");
        TypeInfo::new(name, self.get_type_hash())
    }
}
