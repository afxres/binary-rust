use std::{any::TypeId, collections::HashMap, sync::Arc};

pub struct Generator {
    converters: HashMap<TypeId, Arc<dyn crate::Converter>>,
}

impl Generator {
    pub fn new() -> Self {
        Self { converters: HashMap::new() }
    }
}

impl crate::Generator for Generator {
    fn add_converter(&mut self, converter: &std::sync::Arc<dyn crate::Converter>) {
        self.converters.insert(converter.generic_argument(), converter.clone());
    }

    fn get_converter(&self, r#type: std::any::TypeId) -> Option<std::sync::Arc<dyn crate::Converter>> {
        if let Some(converter) = self.converters.get(&r#type) { Some(converter.clone()) } else { None }
    }
}
