#[macro_export]
macro_rules! impl_with_type_info {
    ($type:ident<$t:ident, $s:ident, $b:ident>) => {
        impl<$t, $s, $b> $type<$t, $s, $b> {
            pub fn with_type_info(mut self, type_info: $crate::entity::TypeInfo) -> Self {
                self.entity.type_info = Some(type_info);
                self
            }
        }
    };
    ($type:ident<$t:ident, $s:ident>) => {
        impl<$t, $s> $type<$t, $s> {
            pub fn with_type_info(mut self, type_info: $crate::entity::TypeInfo) -> Self {
                self.entity.type_info = Some(type_info);
                self
            }
        }
    };
    ($type:ident<$t:ident, $b:ident>) => {
        impl<$t, $b> $type<$t, $b> {
            pub fn with_type_info(mut self, type_info: $crate::entity::TypeInfo) -> Self {
                self.entity.type_info = Some(type_info);
                self
            }
        }
    };
    ($type:ident<$t:ident>) => {
        impl<$t> $type<$t> {
            pub fn with_type_info(mut self, type_info: $crate::entity::TypeInfo) -> Self {
                self.entity.type_info = Some(type_info);
                self
            }
        }
    };
}
