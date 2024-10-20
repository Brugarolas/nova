// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::marker::PhantomData;

use crate::ecmascript::execution::Agent;

/// # ZST type representing access to the garbage collector.
///
/// Access to a garbage collected type's heap data should mainly require
/// holding a `ContextRef<'gc, GcToken>`. Borrowing the heap data should bind
/// to the `'gc` lifetime.
// Note: non-exhaustive to make sure this is not constructable on the outside.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub(crate) struct GcToken;

/// # ZST type representing a JavaScript call scope
///
/// Access to scoped root values should mainly require holding a
/// `ContextRef<'scope, ScopeToken>`. In limited cases, borrowing heap data can
/// bind to the `'scope` lifetime.
// Note: non-exhaustive to make sure this is not constructable on the outside.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScopeToken;

/// An exclusive reference to a token type.
///
/// This is functionally the same as a `&'a mut T` but offers memory efficiency
/// benefits; if `T` is a ZST, this type is as well.
pub(crate) struct ContextMut<'a, T> {
    inner: T,
    _lifetime: PhantomData<&'a mut T>,
}

/// A shared reference to a token type.
///
/// This is functionally the same as a `&'a T` but offers memory efficiency
/// benefits; if `T` is a ZST, this type is as well.
pub(crate) struct ContextRef<'a, T> {
    inner: T,
    _lifetime: PhantomData<&'a T>,
}

#[repr(transparent)]
pub struct Context<'scope, 'gc, 'a> {
    agent: &'a mut Agent,
    scope_ref: ContextRef<'scope, ScopeToken>,
    gc_ref: ContextMut<'gc, GcToken>,
}

impl GcToken {
    #[inline(always)]
    unsafe fn steal() -> Self {
        Self
    }
}

impl ScopeToken {
    #[inline(always)]
    unsafe fn steal() -> Self {
        Self
    }
}

impl<'a, T> ContextMut<'a, T> {
    /// Create a new exclusive reference to a value
    #[inline]
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            _lifetime: PhantomData,
        }
    }

    /// Unsafely clone (duplicate) a singleton.
    ///
    /// # Safety
    ///
    /// This returns an owned clone of the type. You must manually ensure
    /// only one copy is in use at a time.
    ///
    /// You should strongly prefer using `reborrow()` instead. It returns a
    /// `ContextMut` that borrows `self`, which allows the borrow checker
    /// to enforce this at compile time.
    pub(crate) unsafe fn clone_unchecked(&mut self) -> ContextMut<'a, T>
    where
        T: ContextMarkerMut<M = T>,
    {
        ContextMut::new(self.inner.clone_unchecked_mut())
    }

    /// Reborrow into a "child" ContextMut.
    ///
    /// `self` will stay borrowed until the child ContextMut is dropped.
    pub(crate) fn reborrow_mut(&mut self) -> ContextMut<'_, T>
    where
        T: ContextMarkerMut<M = T>,
    {
        // safety: we're returning the clone inside a new ContextMut that borrows
        // self, so user code can't use both at the same time.
        ContextMut::new(unsafe { self.inner.clone_unchecked_mut() })
    }

    /// Reborrow into a "child" ContextRef.
    ///
    /// `self` will stay borrowed until the child ContextRef is dropped.
    pub(crate) fn reborrow(&self) -> ContextRef<'_, T>
    where
        T: ContextMarkerRef<M = T>,
    {
        // safety: we're returning the clone inside a new ContextMut that borrows
        // self, so user code can't use both at the same time.
        ContextRef::new(unsafe { self.inner.clone_unchecked_ref() })
    }
}

impl<'a, T> ContextRef<'a, T> {
    /// Create a new shared reference to a value
    #[inline]
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            _lifetime: PhantomData,
        }
    }
}

impl<'scope, 'gc, 'a> Context<'scope, 'gc, 'a> {
    pub(crate) fn new(
        agent: &'a mut Agent,
        scope: &'scope mut ScopeToken,
        gc: &'gc mut GcToken,
    ) -> Self {
        Self {
            agent,
            scope_ref: scope.into_ref(),
            gc_ref: gc.into_mut(),
        }
    }
}

pub(crate) trait ContextMarkerMut: Sized {
    /// Marker type
    type M;

    unsafe fn clone_unchecked_mut(&mut self) -> Self::M;

    /// Convert a value into a `ContextMut`.
    ///
    /// When called on an owned `T`, yields a `ContextMut<'static, T>`.
    /// When called on an `&'a mut T`, yields a `ContextMut<'a, T>`.
    #[inline]
    fn into_mut<'a>(mut self) -> ContextMut<'a, Self::M>
    where
        Self: 'a,
    {
        ContextMut::new(unsafe { self.clone_unchecked_mut() })
    }
}

pub(crate) trait ContextMarkerRef: Sized {
    /// Marker type
    type M;

    unsafe fn clone_unchecked_ref(&self) -> Self::M;

    /// Convert a value into a `ContextRef`.
    ///
    /// When called on an owned `T`, yields a `ContextRef<'static, T>`.
    /// When called on an `&'a mut T`, yields a `ContextRef<'a, T>`.
    #[inline]
    fn into_ref<'a>(self) -> ContextRef<'a, Self::M>
    where
        Self: 'a,
    {
        ContextRef::new(unsafe { self.clone_unchecked_ref() })
    }
}

impl<T, M> ContextMarkerMut for &mut T
where
    T: ContextMarkerMut<M = M>,
{
    type M = M;

    unsafe fn clone_unchecked_mut(&mut self) -> Self::M {
        T::clone_unchecked_mut(self)
    }
}

impl<T, M> ContextMarkerRef for &T
where
    T: ContextMarkerRef<M = M>,
{
    type M = M;

    unsafe fn clone_unchecked_ref(&self) -> Self::M {
        T::clone_unchecked_ref(self)
    }
}

impl ContextMarkerMut for GcToken {
    type M = Self;

    #[inline]
    unsafe fn clone_unchecked_mut(&mut self) -> Self::M {
        GcToken
    }
}

impl ContextMarkerMut for ScopeToken {
    type M = Self;

    #[inline]
    unsafe fn clone_unchecked_mut(&mut self) -> Self::M {
        ScopeToken
    }
}

impl ContextMarkerRef for GcToken {
    type M = Self;

    #[inline]
    unsafe fn clone_unchecked_ref(&self) -> Self::M {
        GcToken
    }
}

impl ContextMarkerRef for ScopeToken {
    type M = Self;

    #[inline]
    unsafe fn clone_unchecked_ref(&self) -> Self::M {
        ScopeToken
    }
}
