use std::ops::Deref;

/// A wrapper for a value which might be owned or borrowed
/// The key difference from Cow, is that this doesn't require the value to implement Clone
#[allow(unused)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_owned_deref() {
        let owned: MaybeOwned<String> = MaybeOwned::Owned("hello".to_string());
        assert_eq!(&*owned, "hello");
    }

    #[test]
    fn test_borrowed_deref() {
        let value = "hello".to_string();
        let borrowed: MaybeOwned<String> = MaybeOwned::Borrowed(&value);
        assert_eq!(&*borrowed, "hello");
    }

    #[test]
    fn test_owned_into_owned() {
        let owned: MaybeOwned<String> = MaybeOwned::Owned("hello".to_string());
        let result = owned.into_owned();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_borrowed_into_owned() {
        let value = "hello".to_string();
        let borrowed: MaybeOwned<String> = MaybeOwned::Borrowed(&value);
        let result = borrowed.into_owned();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_deref_with_non_clone_type() {
        // MaybeOwned works with types that don't implement Clone for Deref
        struct NonClone(i32);
        let owned: MaybeOwned<NonClone> = MaybeOwned::Owned(NonClone(42));
        assert_eq!(owned.0, 42);
    }

    #[test]
    fn test_borrowed_with_non_clone_type() {
        struct NonClone(i32);
        let value = NonClone(42);
        let borrowed: MaybeOwned<NonClone> = MaybeOwned::Borrowed(&value);
        assert_eq!(borrowed.0, 42);
    }

    #[test]
    fn test_deref_returns_reference() {
        let owned: MaybeOwned<Vec<i32>> = MaybeOwned::Owned(vec![1, 2, 3]);
        assert_eq!(owned.len(), 3);
        assert_eq!(owned[0], 1);
    }

    #[test]
    fn test_into_owned_clones_borrowed() {
        let original = vec![1, 2, 3];
        let borrowed: MaybeOwned<Vec<i32>> = MaybeOwned::Borrowed(&original);
        let cloned = borrowed.into_owned();
        assert_eq!(cloned, vec![1, 2, 3]);
        // Original is still accessible
        assert_eq!(original, vec![1, 2, 3]);
    }

    #[test]
    fn test_into_owned_moves_owned() {
        let owned: MaybeOwned<String> = MaybeOwned::Owned("test".to_string());
        let result = owned.into_owned();
        assert_eq!(result, "test");
        // owned is moved, cannot be used
    }
}
