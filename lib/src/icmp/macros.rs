macro_rules! __define_types {
    (
        impl $ty:ident {
            $(
                #[doc = $doc:literal]
                $name:ident = $value:expr;
            )*
        }
    ) => {
        impl $ty {
            $(
                #[doc = $doc]
                pub const $name: Self = Self($value);
            )*

            /// Construct an instance from the given value.
            #[inline]
            pub const fn new(value: u8) -> Self {
                Self(value)
            }
        }

        impl fmt::Display for $ty {
            #[inline]
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match *self {
                    $(Self::$name => write!(f, $doc),)*
                    _ => write!(f, "Unknown type: {}", self.0),
                }
            }
        }

        impl fmt::Debug for $ty {
            #[inline]
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match *self {
                    $(Self::$name => write!(f, stringify!($name)),)*
                    _ => write!(f, "UNKNOWN({})", self.0),
                }
            }
        }
    };
}

pub(super) use __define_types as define_types;
