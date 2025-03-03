use binary::{allocator::Allocator, converter::Converter};

struct CustomConstantConverter<T: 'static> {
    _t: std::marker::PhantomData<T>,
}

impl<T> CustomConstantConverter<T> {
    fn new() -> Self {
        Self { _t: std::marker::PhantomData }
    }
}

impl<T> binary::Converter for CustomConstantConverter<T> {
    fn length(&self) -> usize {
        std::mem::size_of::<T>()
    }

    fn generic_argument(&self) -> std::any::TypeId {
        std::any::TypeId::of::<T>()
    }
}

impl<T> Converter<T> for CustomConstantConverter<T> {
    fn encode(&self, allocator: &mut binary::allocator::Allocator, item: &T) -> Result<(), Box<dyn std::error::Error>> {
        allocator.append(unsafe { std::slice::from_raw_parts(std::ptr::from_ref(item).cast::<u8>(), std::mem::size_of::<T>()) })
    }

    fn decode(&self, span: &&[u8]) -> Result<T, Box<dyn std::error::Error>> {
        let part = span.split_at_checked(std::mem::size_of::<T>());
        let head = part.ok_or(Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, "not enough bytes.")))?.0;
        Ok(unsafe { head.as_ptr().cast::<T>().read_unaligned() })
    }
}

fn auto_methods_with_data<T: Eq + std::fmt::Debug + 'static>(source: T, expected: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut allocator = Allocator::new();
    let converter = CustomConstantConverter::<T>::new();
    converter.encode_auto(&mut allocator, &source)?;
    assert_eq!(allocator.length(), expected.len());
    assert_eq!(allocator[..], *expected);
    let mut span = &allocator[..];
    assert_eq!(span.len(), expected.len());
    let actual = converter.decode_auto(&mut span)?;
    assert_eq!(actual, source);
    assert_eq!(span.len(), 0);
    Ok(())
}

#[test]
fn auto_methods() -> Result<(), Box<dyn std::error::Error>> {
    auto_methods_with_data(42i32, &42i32.to_ne_bytes())?;
    auto_methods_with_data(97i64, &97i64.to_ne_bytes())?;
    Ok(())
}
