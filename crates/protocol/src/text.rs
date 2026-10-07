//! Serde support for types whose JSON form is their text form.

/// Implements `Serialize` with `Display` and `Deserialize` with `FromStr`, so a value is
/// serialized as a string and a string that doesn't parse is rejected.
macro_rules! serde_via_str {
    ($($ty:ty),* $(,)?) => {
        $(
            impl serde::Serialize for $ty {
                fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                    s.collect_str(self)
                }
            }

            impl<'de> serde::Deserialize<'de> for $ty {
                fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                    let text = <std::borrow::Cow<'de, str>>::deserialize(d)?;
                    text.parse().map_err(serde::de::Error::custom)
                }
            }
        )*
    };
}

serde_via_str!(
    crate::Hash,
    crate::Address,
    crate::relay::RelayIdentity,
    crate::tracker::ErrorCode,
);
