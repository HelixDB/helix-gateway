use std::ops::Deref;

/// A wrapper for a value which might be owned or borrowed
/// The key difference from Cow, is that this doesn't require the value to implement Clone
pub enum MaybeOwned<'a, T> {
    Owned(T),
    Borrowed(&'a T),
}

impl<'a, T> Deref for MaybeOwned<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            MaybeOwned::Owned(v) => v,
            MaybeOwned::Borrowed(v) => v,
        }
    }
}

impl<'a, T: Clone> MaybeOwned<'a, T> {
    pub fn into_owned(self) -> T {
        match self {
            MaybeOwned::Owned(v) => v,
            MaybeOwned::Borrowed(v) => v.clone(),
        }
    }
}
