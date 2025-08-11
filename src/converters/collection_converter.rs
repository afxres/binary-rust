use crate::converter::Converter;

struct CollectionIterator<'a, E> {
    span: &'a [u8],
    converter: &'a dyn Converter<E>,
    first_error: &'a mut Option<Box<dyn std::error::Error>>,
}

impl<'a, E> Iterator for CollectionIterator<'a, E> {
    type Item = E;

    fn next(&mut self) -> Option<Self::Item> {
        if self.span.is_empty() || self.first_error.is_some() {
            None
        } else {
            match self.converter.decode_auto(&mut self.span) {
                Ok(item) => Some(item),
                Err(error) => {
                    *self.first_error = Some(error);
                    None
                }
            }
        }
    }
}

pub struct CollectionConverter<T: IntoIterator + 'static> {
    _t: std::marker::PhantomData<T>,
    converter: Box<dyn Converter<<T as IntoIterator>::Item>>,
}

impl<T: IntoIterator> CollectionConverter<T> {
    pub fn new(converter: Box<dyn Converter<<T as IntoIterator>::Item>>) -> Self {
        Self {
            _t: std::marker::PhantomData,
            converter: converter,
        }
    }
}

impl<T: IntoIterator> crate::interface::Converter for CollectionConverter<T> {
    fn length(&self) -> usize {
        0
    }

    fn generic_argument(&self) -> std::any::TypeId {
        std::any::TypeId::of::<T>()
    }
}

impl<T: IntoIterator + FromIterator<<T as IntoIterator>::Item>> Converter<T> for CollectionConverter<T>
where
    for<'a> &'a T: IntoIterator<Item = &'a <T as IntoIterator>::Item>,
{
    fn encode(&self, allocator: &mut crate::allocator::Allocator, item: &T) -> Result<(), Box<dyn std::error::Error>> {
        for i in item {
            self.converter.encode_auto(allocator, &i)?;
        }
        Ok(())
    }

    fn decode(&self, span: &&[u8]) -> Result<T, Box<dyn std::error::Error>> {
        let mut first_error = None;
        let iterator = CollectionIterator {
            span: span,
            converter: &*self.converter,
            first_error: &mut first_error,
        };
        let result: T = iterator.collect();
        if let Some(error) = first_error { Err(error) } else { Ok(result) }
    }
}
