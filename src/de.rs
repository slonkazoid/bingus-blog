use std::num::NonZeroU64;

use serde::de::Error;

use serde::{
    de::{Unexpected, Visitor},
    Deserializer,
};

pub struct RangedI64Visitor<const START: i64, const END: i64>;
impl<const START: i64, const END: i64> serde::de::Visitor<'_> for RangedI64Visitor<START, END> {
    type Value = i64;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "an integer between {START} and {END}")
    }

    fn visit_i32<E>(self, v: i32) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_i64(v as i64)
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
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

    fn visit_i128<E>(self, v: i128) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_i64(v as i64)
    }
}

pub struct U64Visitor;
impl Visitor<'_> for U64Visitor {
    type Value = u64;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "a non-negative integer")
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v)
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        u64::try_from(v).map_err(|_| E::invalid_value(Unexpected::Signed(v), &self))
    }
}

pub struct MillisVisitor;
impl<'de> Visitor<'de> for MillisVisitor {
    type Value = Option<NonZeroU64>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "a positive integer")
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        let n = deserializer.deserialize_i64(U64Visitor)?;
        NonZeroU64::new(n)
            .ok_or(D::Error::invalid_value(Unexpected::Unsigned(n), &self))
            .map(Some)
    }
}
