use crate::internal::{error_helper, length};
use std::alloc::Layout;

pub struct Allocator {
    allocated: bool,
    buffer: *mut u8,
    offset: usize,
    bounds: usize,
    limits: usize,
}

impl Allocator {
    const ANCHOR_SIZE: usize = 4;
    const ANCHOR_SHRINK_LIMITS: usize = 16;

    pub fn new() -> Self {
        Self {
            allocated: false,
            buffer: std::ptr::null_mut(),
            offset: 0,
            bounds: 0,
            limits: i32::MAX as usize,
        }
    }

    pub fn length(&self) -> usize {
        self.offset
    }

    pub fn capacity(&self) -> usize {
        self.bounds
    }

    pub fn max_capacity(&self) -> usize {
        self.limits
    }

    fn resize(&mut self, length: usize) -> Result<(), Box<dyn std::error::Error>> {
        assert!(self.limits <= i32::MAX as usize);
        assert!(self.bounds <= self.limits);
        assert!(self.offset <= self.bounds);
        assert!(length != 0);

        let offset = self.offset;
        let limits = self.limits;
        let amount = offset as u64 + length as u64;
        if length > i32::MAX as usize || amount > limits as u64 {
            return Err(error_helper::error_allocator_max_capacity_overflow());
        }

        let source = self.bounds;
        let mut cursor = source as u64;
        if cursor == 0 {
            cursor = 256;
        }
        while cursor < amount {
            cursor *= 2;
        }
        if cursor > limits as u64 {
            cursor = limits as u64;
        }
        assert!(amount <= cursor);
        assert!(cursor <= self.limits as u64);

        let bounds = cursor as usize;
        let target = unsafe { std::alloc::alloc(Layout::from_size_align(bounds, 1).unwrap()) };
        if target.is_null() {
            return Err(error_helper::error_allocator_allocate_failed());
        }
        if self.allocated {
            assert!(self.buffer.is_null() == false);
            unsafe { std::alloc::dealloc(self.buffer, Layout::from_size_align(self.bounds, 1).unwrap()) };
        }
        self.allocated = true;
        self.buffer = target;
        self.bounds = bounds;
        assert!(offset <= source);
        assert!(offset <= self.bounds);
        Ok(())
    }

    pub fn ensure(&mut self, length: usize) -> Result<(), Box<dyn std::error::Error>> {
        assert!(self.bounds <= i32::MAX as usize);
        assert!(self.offset <= self.bounds);
        if length > i32::MAX as usize || self.offset as u64 + length as u64 > self.bounds as u64 {
            self.resize(length)?;
        }
        assert!(self.bounds <= self.limits);
        assert!(self.bounds >= self.offset + length);
        Ok(())
    }

    pub(crate) fn assign(&mut self, length: usize) -> Result<*mut u8, Box<dyn std::error::Error>> {
        assert!(length != 0);
        self.ensure(length)?;
        let offset = self.offset;
        self.offset = offset + length;
        Ok(unsafe { self.buffer.add(offset) })
    }

    pub(crate) fn anchor(&mut self) -> Result<usize, Box<dyn std::error::Error>> {
        self.ensure(Self::ANCHOR_SIZE)?;
        let offset = self.offset;
        self.offset = offset + Self::ANCHOR_SIZE;
        Ok(offset)
    }

    pub(crate) fn finish_anchor(&mut self, anchor: usize) -> Result<(), Box<dyn std::error::Error>> {
        assert!(self.bounds <= i32::MAX as usize);
        assert!(self.offset <= self.bounds);
        let offset = self.offset;
        let refers = anchor as u64 + Self::ANCHOR_SIZE as u64;
        if anchor > i32::MAX as usize || refers > offset as u64 {
            return Err(error_helper::error_allocator_invalid());
        }
        let length = offset - refers as usize;
        let target = unsafe { self.buffer.add(anchor) };
        if length <= Self::ANCHOR_SHRINK_LIMITS {
            self.offset = offset - 3;
            unsafe { length::encode_length_prefix(target, length, 1) };
            unsafe { std::ptr::copy(target.add(4), target.add(1), length) };
            assert!(self.offset >= 1);
            assert!(self.offset <= self.bounds);
        } else {
            unsafe { length::encode_length_prefix(target, length, 4) };
            assert!(self.offset >= 4);
            assert!(self.offset <= self.bounds);
        }
        Ok(())
    }

    pub fn append(&mut self, span: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        if span.is_empty() {
            return Ok(());
        }
        unsafe {
            std::ptr::copy(span.as_ptr(), self.assign(span.len())?, span.len());
        }
        Ok(())
    }
}

impl Drop for Allocator {
    fn drop(&mut self) {
        if self.allocated {
            assert!(self.offset != 0);
            assert!(self.bounds >= self.offset);
            assert!(self.buffer.is_null() == false);
            unsafe { std::alloc::dealloc(self.buffer, Layout::from_size_align(self.bounds, 1).unwrap()) };
        }
    }
}

impl std::fmt::Display for Allocator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "length = {}, capacity = {}, max capacity = {}", self.length(), self.capacity(), self.max_capacity())
    }
}

impl<Index> std::ops::Index<Index> for Allocator
where
    Index: std::slice::SliceIndex<[u8]>,
{
    type Output = Index::Output;

    fn index(&self, index: Index) -> &Self::Output {
        if self.offset == 0 {
            assert!(self.buffer.is_null());
            return &[][index];
        } else {
            assert!(self.buffer.is_null() == false);
            return &(unsafe { std::slice::from_raw_parts(self.buffer, self.offset) })[index];
        }
    }
}
