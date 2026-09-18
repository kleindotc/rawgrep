pub trait Unwrap_<T> {
    fn unwrap_(self) -> T;
}

impl<T> Unwrap_<T> for std::option::Option<T> {
    #[track_caller]
    #[inline]
    fn unwrap_(self) -> T {
        #[cfg(debug_assertions)]             { self.unwrap() }
        #[cfg(not(debug_assertions))] unsafe { self.unwrap_unchecked() }
    }
}

impl<T, E> Unwrap_<T> for std::result::Result<T, E>
where
    E: std::fmt::Debug,
{
    #[track_caller]
    #[inline]
    fn unwrap_(self) -> T {
        #[cfg(debug_assertions)]             { self.unwrap() }
        #[cfg(not(debug_assertions))] unsafe { self.unwrap_unchecked() }
    }
}
