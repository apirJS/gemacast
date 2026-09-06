pub trait OutputVolumeReader: Send + Sync + 'static {
    fn read(&self) -> Option<f32>;
}

#[cfg(feature = "dynamic-dispatch")]
impl OutputVolumeReader for Box<dyn OutputVolumeReader> {
    fn read(&self) -> Option<f32> {
        (**self).read()
    }
}
