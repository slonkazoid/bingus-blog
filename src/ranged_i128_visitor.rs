pub struct RangedI128Visitor<const START: i128, const END: i128>;
impl<'de, const START: i128, const END: i128> serde::de::Visitor<'de>
    for RangedI128Visitor<START, END>
{
    type Value = i128;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "an integer between {START} and {END}")
    }

    fn visit_i32<E>(self, v: i32) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_i128(v as i128)
    }

    fn visit_i64<E>(self, v: i64) -> std::prelude::v1::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_i128(v as i128)
    }

    fn visit_i128<E>(self, v: i128) -> std::prelude::v1::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if v >= START && v <= END {
            Ok(v)
        } else {
            Err(E::custom(format!(
                "integer is out of bounds ({START}..{END})"
            )))
        }
    }
}
