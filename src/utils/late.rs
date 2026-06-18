use std::mem::MaybeUninit;
use std::ops::{Deref, DerefMut};

pub struct Late<T>{
    maybe_uninit: MaybeUninit<T>,
    has_init: bool
}

impl<T> Default for Late<T> {
    fn default() -> Self {
        Self::uninit()
    }
}

impl<T> Late<T> {
    pub const fn uninit() -> Self {
        Late {
            maybe_uninit: MaybeUninit::uninit(),
            has_init: false,
        }
    }

    pub fn init(&mut self, val: T) {
        self.maybe_uninit.write(val);
        self.has_init = true;
    }

    pub fn is_init(&self) -> bool {
        self.has_init
    }  
}

impl<T> Deref for Late<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: caller guarantees init
        unsafe { self.maybe_uninit.assume_init_ref() }
    }
}

impl<T> DerefMut for Late<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { self.maybe_uninit.assume_init_mut() }
    }
}