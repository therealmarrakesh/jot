//! Minimal stack-inline `SmallVec<[T; N]>` (replaces the external `smallvec` crate).

use std::borrow::{Borrow, BorrowMut};
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::mem::MaybeUninit;
use std::ops::{Deref, DerefMut, Index, IndexMut};
use std::ptr;
use std::slice;

pub(crate) trait Array {
    type Item;
    const CAPACITY: usize;
}

impl<T, const N: usize> Array for [T; N] {
    type Item = T;
    const CAPACITY: usize = N;
}

enum Repr<A: Array> {
    Inline { len: usize, data: MaybeUninit<A> },
    Heap(Vec<A::Item>),
}

pub(crate) struct SmallVec<A: Array> {
    repr: Repr<A>,
}

impl<A: Array> SmallVec<A> {
    #[inline]
    pub(crate) const fn new_const() -> Self {
        Self {
            repr: Repr::Inline {
                len: 0,
                data: MaybeUninit::uninit(),
            },
        }
    }

    #[inline]
    pub(crate) fn new() -> Self {
        Self::new_const()
    }

    #[inline]
    pub(crate) fn with_capacity(n: usize) -> Self {
        if n <= A::CAPACITY {
            Self::new_const()
        } else {
            Self {
                repr: Repr::Heap(Vec::with_capacity(n)),
            }
        }
    }

    #[inline]
    pub(crate) fn from_vec(vec: Vec<A::Item>) -> Self {
        Self {
            repr: Repr::Heap(vec),
        }
    }

    #[inline]
    pub(crate) fn from_buf(buf: A) -> Self {
        Self {
            repr: Repr::Inline {
                len: A::CAPACITY,
                data: MaybeUninit::new(buf),
            },
        }
    }

    #[inline]
    pub(crate) fn from_slice(slice: &[A::Item]) -> Self
    where
        A::Item: Clone,
    {
        let mut v = Self::with_capacity(slice.len());
        v.extend_from_slice(slice);
        v
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        match &self.repr {
            Repr::Inline { len, .. } => *len,
            Repr::Heap(v) => v.len(),
        }
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub(crate) fn as_slice(&self) -> &[A::Item] {
        match &self.repr {
            Repr::Inline { len, data } => unsafe {
                slice::from_raw_parts(data.as_ptr() as *const A::Item, *len)
            },
            Repr::Heap(v) => v.as_slice(),
        }
    }

    #[inline]
    pub(crate) fn as_mut_slice(&mut self) -> &mut [A::Item] {
        match &mut self.repr {
            Repr::Inline { len, data } => unsafe {
                slice::from_raw_parts_mut(data.as_mut_ptr() as *mut A::Item, *len)
            },
            Repr::Heap(v) => v.as_mut_slice(),
        }
    }

    fn spill(&mut self, min_cap: usize) -> &mut Vec<A::Item> {
        let cap = min_cap.max(A::CAPACITY.saturating_mul(2)).max(4);
        if let Repr::Inline { len, data } = &mut self.repr {
            let count = *len;
            *len = 0;
            let mut vec = Vec::with_capacity(cap);
            unsafe {
                ptr::copy_nonoverlapping(
                    data.as_ptr() as *const A::Item,
                    vec.as_mut_ptr(),
                    count,
                );
                vec.set_len(count);
            }
            self.repr = Repr::Heap(vec);
        }
        match &mut self.repr {
            Repr::Heap(v) => {
                if v.capacity() < min_cap {
                    v.reserve(min_cap - v.len());
                }
                v
            }
            Repr::Inline { .. } => unreachable!(),
        }
    }

    #[inline]
    pub(crate) fn reserve(&mut self, additional: usize) {
        let needed = self.len().saturating_add(additional);
        match &mut self.repr {
            Repr::Inline { .. } => {
                if needed > A::CAPACITY {
                    self.spill(needed);
                }
            }
            Repr::Heap(v) => v.reserve(additional),
        }
    }

    #[inline]
    pub(crate) fn push(&mut self, value: A::Item) {
        match &mut self.repr {
            Repr::Inline { len, data } if *len < A::CAPACITY => unsafe {
                let ptr = (data.as_mut_ptr() as *mut A::Item).add(*len);
                ptr.write(value);
                *len += 1;
            },
            Repr::Heap(v) => v.push(value),
            _ => {
                let needed = self.len() + 1;
                self.spill(needed).push(value);
            }
        }
    }

    #[inline]
    pub(crate) fn pop(&mut self) -> Option<A::Item> {
        match &mut self.repr {
            Repr::Inline { len, data } => {
                if *len == 0 {
                    None
                } else {
                    *len -= 1;
                    Some(unsafe { (data.as_ptr() as *const A::Item).add(*len).read() })
                }
            }
            Repr::Heap(v) => v.pop(),
        }
    }

    pub(crate) fn remove(&mut self, index: usize) -> A::Item {
        match &mut self.repr {
            Repr::Inline { len, data } => {
                assert!(index < *len, "removal index out of bounds");
                unsafe {
                    let base = data.as_mut_ptr() as *mut A::Item;
                    let item = base.add(index).read();
                    ptr::copy(base.add(index + 1), base.add(index), *len - index - 1);
                    *len -= 1;
                    item
                }
            }
            Repr::Heap(v) => v.remove(index),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.truncate(0);
    }

    pub(crate) fn truncate(&mut self, new_len: usize) {
        match &mut self.repr {
            Repr::Inline { len, data } => {
                if new_len < *len {
                    let old_len = *len;
                    *len = new_len;
                    unsafe {
                        let base = data.as_mut_ptr() as *mut A::Item;
                        ptr::drop_in_place(ptr::slice_from_raw_parts_mut(
                            base.add(new_len),
                            old_len - new_len,
                        ));
                    }
                }
            }
            Repr::Heap(v) => v.truncate(new_len),
        }
    }

    pub(crate) fn extend_from_slice(&mut self, other: &[A::Item])
    where
        A::Item: Clone,
    {
        self.reserve(other.len());
        for item in other {
            self.push(item.clone());
        }
    }

    pub(crate) fn into_vec(mut self) -> Vec<A::Item> {
        match &mut self.repr {
            Repr::Heap(v) => std::mem::take(v),
            Repr::Inline { len, data } => {
                let count = *len;
                *len = 0;
                let mut vec = Vec::with_capacity(count);
                unsafe {
                    ptr::copy_nonoverlapping(
                        data.as_ptr() as *const A::Item,
                        vec.as_mut_ptr(),
                        count,
                    );
                    vec.set_len(count);
                }
                vec
            }
        }
    }
}

impl<A: Array> Drop for SmallVec<A> {
    fn drop(&mut self) {
        if let Repr::Inline { len, data } = &mut self.repr {
            let count = *len;
            *len = 0;
            unsafe {
                ptr::drop_in_place(ptr::slice_from_raw_parts_mut(
                    data.as_mut_ptr() as *mut A::Item,
                    count,
                ));
            }
        }
    }
}

impl<A: Array> Default for SmallVec<A> {
    #[inline]
    fn default() -> Self {
        Self::new_const()
    }
}

impl<A: Array> Clone for SmallVec<A>
where
    A::Item: Clone,
{
    fn clone(&self) -> Self {
        Self::from_slice(self.as_slice())
    }
}

impl<A: Array> Deref for SmallVec<A> {
    type Target = [A::Item];
    #[inline]
    fn deref(&self) -> &[A::Item] {
        self.as_slice()
    }
}

impl<A: Array> DerefMut for SmallVec<A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [A::Item] {
        self.as_mut_slice()
    }
}

impl<A: Array> AsRef<[A::Item]> for SmallVec<A> {
    #[inline]
    fn as_ref(&self) -> &[A::Item] {
        self.as_slice()
    }
}

impl<A: Array> AsMut<[A::Item]> for SmallVec<A> {
    #[inline]
    fn as_mut(&mut self) -> &mut [A::Item] {
        self.as_mut_slice()
    }
}

impl<A: Array> Borrow<[A::Item]> for SmallVec<A> {
    #[inline]
    fn borrow(&self) -> &[A::Item] {
        self.as_slice()
    }
}

impl<A: Array> BorrowMut<[A::Item]> for SmallVec<A> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut [A::Item] {
        self.as_mut_slice()
    }
}

impl<A: Array, I: slice::SliceIndex<[A::Item]>> Index<I> for SmallVec<A> {
    type Output = I::Output;
    #[inline]
    fn index(&self, index: I) -> &Self::Output {
        &self.as_slice()[index]
    }
}

impl<A: Array, I: slice::SliceIndex<[A::Item]>> IndexMut<I> for SmallVec<A> {
    #[inline]
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        &mut self.as_mut_slice()[index]
    }
}

impl<A: Array> fmt::Debug for SmallVec<A>
where
    A::Item: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_slice(), f)
    }
}

impl<A: Array, B: Array> PartialEq<SmallVec<B>> for SmallVec<A>
where
    A::Item: PartialEq<B::Item>,
{
    #[inline]
    fn eq(&self, other: &SmallVec<B>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<A: Array> PartialEq<[A::Item]> for SmallVec<A>
where
    A::Item: PartialEq,
{
    #[inline]
    fn eq(&self, other: &[A::Item]) -> bool {
        self.as_slice() == other
    }
}

impl<A: Array, const N: usize> PartialEq<[A::Item; N]> for SmallVec<A>
where
    A::Item: PartialEq,
{
    #[inline]
    fn eq(&self, other: &[A::Item; N]) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<A: Array> Eq for SmallVec<A> where A::Item: Eq {}

impl<A: Array> PartialOrd for SmallVec<A>
where
    A::Item: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.as_slice().partial_cmp(other.as_slice())
    }
}

impl<A: Array> Ord for SmallVec<A>
where
    A::Item: Ord,
{
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl<A: Array> Hash for SmallVec<A>
where
    A::Item: Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl<A: Array> From<Vec<A::Item>> for SmallVec<A> {
    fn from(vec: Vec<A::Item>) -> Self {
        Self::from_vec(vec)
    }
}

impl<A: Array> From<&[A::Item]> for SmallVec<A>
where
    A::Item: Clone,
{
    fn from(slice: &[A::Item]) -> Self {
        Self::from_slice(slice)
    }
}

impl<T, const N: usize> From<[T; N]> for SmallVec<[T; N]> {
    fn from(buf: [T; N]) -> Self {
        Self::from_buf(buf)
    }
}

impl<A: Array> FromIterator<A::Item> for SmallVec<A> {
    fn from_iter<I: IntoIterator<Item = A::Item>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        let mut v = Self::with_capacity(lower);
        for item in iter {
            v.push(item);
        }
        v
    }
}

impl<A: Array> Extend<A::Item> for SmallVec<A> {
    fn extend<I: IntoIterator<Item = A::Item>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        self.reserve(lower);
        for item in iter {
            self.push(item);
        }
    }
}

impl<A: Array> IntoIterator for SmallVec<A> {
    type Item = A::Item;
    type IntoIter = std::vec::IntoIter<A::Item>;
    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}

impl<'a, A: Array> IntoIterator for &'a SmallVec<A> {
    type Item = &'a A::Item;
    type IntoIter = slice::Iter<'a, A::Item>;
    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

impl<'a, A: Array> IntoIterator for &'a mut SmallVec<A> {
    type Item = &'a mut A::Item;
    type IntoIter = slice::IterMut<'a, A::Item>;
    fn into_iter(self) -> Self::IntoIter {
        self.as_mut_slice().iter_mut()
    }
}

macro_rules! smallvec {
    () => {
        $crate::ui::smallvec::SmallVec::new()
    };
    ($($x:expr),+ $(,)?) => {{
        let mut v = $crate::ui::smallvec::SmallVec::new();
        $(v.push($x);)+
        v
    }};
}

pub(crate) use smallvec;
