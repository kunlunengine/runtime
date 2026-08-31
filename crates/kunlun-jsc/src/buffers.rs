//! Fixed ArrayBuffers and checked TypedArray views; no borrowed engine memory.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum TypedArrayKind {
    Int8 = sys::KUNLUN_JSC_ARRAY_INT8,
    Uint8 = sys::KUNLUN_JSC_ARRAY_UINT8,
    Uint8Clamped = sys::KUNLUN_JSC_ARRAY_UINT8_CLAMPED,
    Int16 = sys::KUNLUN_JSC_ARRAY_INT16,
    Uint16 = sys::KUNLUN_JSC_ARRAY_UINT16,
    Int32 = sys::KUNLUN_JSC_ARRAY_INT32,
    Uint32 = sys::KUNLUN_JSC_ARRAY_UINT32,
    Float32 = sys::KUNLUN_JSC_ARRAY_FLOAT32,
    Float64 = sys::KUNLUN_JSC_ARRAY_FLOAT64,
    BigInt64 = sys::KUNLUN_JSC_ARRAY_BIGINT64,
    BigUint64 = sys::KUNLUN_JSC_ARRAY_BIGUINT64,
}

impl TypedArrayKind {
    pub const fn element_size(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 | Self::Uint8Clamped => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 | Self::BigInt64 | Self::BigUint64 => 8,
        }
    }
}

/// A rooted fixed-length ArrayBuffer with independent native backing storage.
/// Construction copies Rust bytes. GC frees only that native allocation, even
/// after Rust handles disappear or JS transfers ownership. All access copies
/// bytes, never borrows engine storage. JSC's C API pins storage on nonempty
/// reads/writes, so later JavaScript `transfer()` may throw.
///
/// ```compile_fail
/// use kunlun_jsc::ArrayBuffer;
/// fn send<T: Send>() {}
/// send::<ArrayBuffer<'static>>();
/// ```
/// ```compile_fail
/// use kunlun_jsc::ArrayBuffer;
/// fn sync<T: Sync>() {}
/// sync::<ArrayBuffer<'static>>();
/// ```
/// ```compile_fail
/// use kunlun_jsc::JscVm;
/// let buffer = { let vm = JscVm::new("temporary").unwrap(); vm.array_buffer(&[]).unwrap() };
/// buffer.len().unwrap();
/// ```
#[derive(Clone)]
pub struct ArrayBuffer<'context> {
    value: RootedValue<'context>,
}

/// A rooted TypedArray with a checked element kind, byte offset and element
/// count. It retains the backing buffer independently of the original handle.
/// Access is in bytes (native endian), avoiding unchecked scalar pointer casts.
///
/// ```compile_fail
/// use kunlun_jsc::TypedArray;
/// fn send<T: Send>() {}
/// send::<TypedArray<'static>>();
/// ```
/// ```compile_fail
/// use kunlun_jsc::TypedArray;
/// fn sync<T: Sync>() {}
/// sync::<TypedArray<'static>>();
/// ```
/// ```compile_fail
/// use kunlun_jsc::{JscVm, TypedArrayKind};
/// let view = {
///     let vm = JscVm::new("temporary").unwrap();
///     vm.array_buffer(&[0; 8]).unwrap().typed_array(TypedArrayKind::Uint8, 0, 8).unwrap()
/// };
/// view.len().unwrap();
/// ```
#[derive(Clone)]
pub struct TypedArray<'context> {
    value: RootedValue<'context>,
    buffer: ArrayBuffer<'context>,
    kind: TypedArrayKind,
    byte_offset: usize,
    length: usize,
}

fn check(
    context: &ContextInner,
    operation: &'static str,
    status: u32,
    exception: ValueRef,
) -> Result<(), JscError> {
    if !exception.is_null() {
        return Err(context.exception_error(operation, None, exception));
    }
    expect_status(operation, status)
}

impl JscVm {
    pub fn array_buffer(&self, bytes: &[u8]) -> Result<ArrayBuffer<'_>, JscError> {
        let mut buffer = ptr::null_mut();
        let mut exception = ptr::null();
        // SAFETY: the shim copies the borrowed bytes before returning; outputs
        // are writable and the resulting object belongs to this live context.
        let status = unsafe {
            sys::kunlun_jsc_array_buffer_create_copy(
                self.context.as_context(),
                bytes.as_ptr(),
                bytes.len() as u64,
                &mut buffer,
                &mut exception,
            )
        };
        check(&self.context, "array_buffer_create", status, exception)?;
        Ok(ArrayBuffer {
            value: RootedValue {
                protected: ProtectedValue::new(
                    Rc::clone(&self.context),
                    buffer,
                    "array_buffer_create",
                )?,
                source_url: None,
                _owner: PhantomData,
            },
        })
    }

    /// Requests a GC cycle; JSC may defer collection of conservatively rooted
    /// objects. This is not an exact or synchronous finalization guarantee.
    pub fn collect_garbage(&self) -> Result<(), JscError> {
        // SAFETY: this live context is confined to the current thread.
        expect_status("collect_garbage", unsafe {
            sys::kunlun_jsc_context_collect_garbage(self.context.as_context())
        })
    }
}

impl<'context> ArrayBuffer<'context> {
    pub fn set_global(&self, name: &str) -> Result<(), JscError> {
        self.value.set_global(name)
    }

    /// Detached buffers return a structured JavaScript exception, not zero.
    pub fn len(&self) -> Result<usize, JscError> {
        let mut length = 0;
        let mut exception = ptr::null();
        let context = self.value.protected.context();
        // SAFETY: both handles are retained and thread-affine.
        let status = unsafe {
            sys::kunlun_jsc_array_buffer_length(
                context.as_context(),
                self.value.protected.as_object(),
                &mut length,
                &mut exception,
            )
        };
        check(context, "array_buffer_length", status, exception)?;
        usize::try_from(length).map_err(|_| {
            JscError::native(
                "array_buffer_length",
                sys::KUNLUN_JSC_STATUS_INTEGER_OVERFLOW,
            )
        })
    }

    pub fn is_empty(&self) -> Result<bool, JscError> {
        Ok(self.len()? == 0)
    }

    pub fn read(&self, byte_offset: usize, bytes: &mut [u8]) -> Result<(), JscError> {
        let mut exception = ptr::null();
        let context = self.value.protected.context();
        // SAFETY: destination is an independent Rust slice; the shim checks
        // detachment/range and copies synchronously without exposing storage.
        let status = unsafe {
            sys::kunlun_jsc_array_buffer_read(
                context.as_context(),
                self.value.protected.as_object(),
                byte_offset as u64,
                bytes.as_mut_ptr(),
                bytes.len() as u64,
                &mut exception,
            )
        };
        check(context, "array_buffer_read", status, exception)
    }

    pub fn write(&self, byte_offset: usize, bytes: &[u8]) -> Result<(), JscError> {
        let mut exception = ptr::null();
        let context = self.value.protected.context();
        // SAFETY: source is independently borrowed for this synchronous copy.
        let status = unsafe {
            sys::kunlun_jsc_array_buffer_write(
                context.as_context(),
                self.value.protected.as_object(),
                byte_offset as u64,
                bytes.as_ptr(),
                bytes.len() as u64,
                &mut exception,
            )
        };
        check(context, "array_buffer_write", status, exception)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, JscError> {
        let mut bytes = allocate(self.len()?)?;
        self.read(0, &mut bytes)?;
        Ok(bytes)
    }

    pub fn typed_array(
        &self,
        kind: TypedArrayKind,
        byte_offset: usize,
        length: usize,
    ) -> Result<TypedArray<'context>, JscError> {
        let mut array = ptr::null_mut();
        let mut exception = ptr::null();
        let context = self.value.protected.context();
        // SAFETY: shim validates kind, alignment, detachment and element count
        // before creating a view; the backing buffer is rooted throughout.
        let status = unsafe {
            sys::kunlun_jsc_typed_array_create(
                context.as_context(),
                self.value.protected.as_object(),
                kind as u32,
                byte_offset as u64,
                length as u64,
                &mut array,
                &mut exception,
            )
        };
        check(context, "typed_array_create", status, exception)?;
        Ok(TypedArray {
            value: RootedValue {
                protected: ProtectedValue::new(Rc::clone(context), array, "typed_array_create")?,
                source_url: None,
                _owner: PhantomData,
            },
            buffer: self.clone(),
            kind,
            byte_offset,
            length,
        })
    }
}

fn allocate(length: usize) -> Result<Vec<u8>, JscError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| JscError::native("buffer_copy", sys::KUNLUN_JSC_STATUS_OUT_OF_MEMORY))?;
    bytes.resize(length, 0);
    Ok(bytes)
}

impl TypedArray<'_> {
    pub fn set_global(&self, name: &str) -> Result<(), JscError> {
        self.value.set_global(name)
    }
    pub const fn kind(&self) -> TypedArrayKind {
        self.kind
    }
    pub const fn byte_offset(&self) -> usize {
        self.byte_offset
    }

    pub fn len(&self) -> Result<usize, JscError> {
        self.buffer.len()?; // Detect JS detachment on every access.
        Ok(self.length)
    }
    pub fn is_empty(&self) -> Result<bool, JscError> {
        Ok(self.len()? == 0)
    }
    pub fn byte_len(&self) -> Result<usize, JscError> {
        Ok(self.len()? * self.kind.element_size())
    }

    pub fn read(&self, byte_offset: usize, bytes: &mut [u8]) -> Result<(), JscError> {
        self.check_range(byte_offset, bytes.len())?;
        self.buffer.read(self.byte_offset + byte_offset, bytes)
    }
    pub fn write(&self, byte_offset: usize, bytes: &[u8]) -> Result<(), JscError> {
        self.check_range(byte_offset, bytes.len())?;
        self.buffer.write(self.byte_offset + byte_offset, bytes)
    }
    pub fn to_vec(&self) -> Result<Vec<u8>, JscError> {
        let mut bytes = allocate(self.byte_len()?)?;
        self.read(0, &mut bytes)?;
        Ok(bytes)
    }
    fn check_range(&self, offset: usize, count: usize) -> Result<(), JscError> {
        let length = self.byte_len()?;
        if offset > length || count > length - offset {
            return Err(JscError::native(
                "typed_array_range",
                sys::KUNLUN_JSC_STATUS_OUT_OF_BOUNDS,
            ));
        }
        Ok(())
    }
}
