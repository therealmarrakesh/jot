use std::any::Any;

/// Implements [`Action`] for a type that implements `Clone + PartialEq + Send + 'static`.
#[macro_export]
macro_rules! impl_action {
    ($namespace:path, $name:ident) => {
        impl $crate::Action for $name {
            fn boxed_clone(&self) -> ::std::boxed::Box<dyn $crate::Action> {
                ::std::boxed::Box::new(::std::clone::Clone::clone(self))
            }

            fn partial_eq(&self, action: &dyn $crate::Action) -> bool {
                action
                    .as_any()
                    .downcast_ref::<Self>()
                    .is_some_and(|other| self == other)
            }

            fn name(&self) -> &'static str {
                <Self as $crate::Action>::name_for_type()
            }

            fn name_for_type() -> &'static str {
                concat!(stringify!($namespace), "::", stringify!($name))
            }
        }
    };
    ($name:ident) => {
        impl $crate::Action for $name {
            fn boxed_clone(&self) -> ::std::boxed::Box<dyn $crate::Action> {
                ::std::boxed::Box::new(::std::clone::Clone::clone(self))
            }

            fn partial_eq(&self, action: &dyn $crate::Action) -> bool {
                action
                    .as_any()
                    .downcast_ref::<Self>()
                    .is_some_and(|other| self == other)
            }

            fn name(&self) -> &'static str {
                <Self as $crate::Action>::name_for_type()
            }

            fn name_for_type() -> &'static str {
                stringify!($name)
            }
        }
    };
}

/// Defines unit structs that implement [`Action`].
#[macro_export]
macro_rules! actions {
    ($namespace:path, [ $( $(#[$attr:meta])* $name:ident),* $(,)? ]) => {
        $(
            #[derive(::std::clone::Clone, ::std::cmp::PartialEq, ::std::default::Default, ::std::fmt::Debug)]
            $(#[$attr])*
            pub struct $name;
            $crate::impl_action!($namespace, $name);
        )*
    };
    ([ $( $(#[$attr:meta])* $name:ident),* $(,)? ]) => {
        $(
            #[derive(::std::clone::Clone, ::std::cmp::PartialEq, ::std::default::Default, ::std::fmt::Debug)]
            $(#[$attr])*
            pub struct $name;
            $crate::impl_action!($name);
        )*
    };
}

/// Actions are used to implement keyboard-driven UI.
pub trait Action: Any + Send {
    /// Clone the action into a new box.
    fn boxed_clone(&self) -> Box<dyn Action>;

    /// Do a partial equality check on this action and the other.
    fn partial_eq(&self, action: &dyn Action) -> bool;

    /// Get the name of this action, for displaying in UI.
    fn name(&self) -> &'static str;

    /// Get the name of this action type (static).
    fn name_for_type() -> &'static str
    where
        Self: Sized;
}

impl std::fmt::Debug for dyn Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("dyn Action")
            .field("name", &self.name())
            .finish()
    }
}

impl dyn Action {
    /// Type-erase Action type.
    pub fn as_any(&self) -> &dyn Any {
        self as &dyn Any
    }
}
