// Use the associated constant in a public type: rustdoc must resolve it,
// whereas it can skip checking an unused anonymous constant's initializer.
pub type InternalStart = [(); rand_core::Error::INTERNAL_START as usize];
