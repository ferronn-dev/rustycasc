use derive_more::Display;

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
#[display(fmt = "{:032x}", _0)]
pub(crate) struct ContentKey(pub(crate) u128);

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
#[display(fmt = "{:032x}", _0)]
pub(crate) struct EncodingKey(pub(crate) u128);
