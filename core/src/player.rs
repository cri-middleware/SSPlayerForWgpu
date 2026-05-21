use ssruntime::error::SSRuntimeError;
use ssruntime::{Context, SsabData, SsabResource};
use std::sync::Arc;

pub struct Player {
    pub runtime_context: Context<'static>,
    resource: Option<Arc<SsabResource>>,
    current_animation: Option<String>,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub fn new() -> Self {
        Self { runtime_context: Context::new(), resource: None, current_animation: None }
    }

    pub fn load_ssab(&mut self, data: &[u8]) -> Result<(), SSRuntimeError> {
        let ssab_data = SsabData::load_copy(data)?;
        let resource = SsabResource::new(ssab_data);

        let res_ptr = resource.resource_static_ptr();
        if res_ptr.is_null() {
            return Err(SSRuntimeError::NotLoaded);
        }

        self.resource = Some(resource);
        let static_res = unsafe { &*res_ptr };
        self.runtime_context.bind_resource(static_res);
        self.current_animation = None;
        Ok(())
    }

    pub fn play(&mut self, anim_name: &str) -> Result<(), SSRuntimeError> {
        self.runtime_context.setup_animation(anim_name)?;
        self.runtime_context.play();
        self.current_animation = Some(anim_name.to_string());
        Ok(())
    }

    pub fn update(&mut self, delta_seconds: f32) -> f32 {
        self.runtime_context.update(delta_seconds * 1000.0)
    }

    pub fn get_frame_no(&self) -> f32 {
        self.runtime_context.get_frame_no()
    }

    pub fn is_playing(&self) -> bool {
        self.runtime_context.is_playing()
    }

    pub fn is_pausing(&self) -> bool {
        self.runtime_context.is_pausing()
    }

    pub fn pause(&mut self) {
        self.runtime_context.pause();
    }

    pub fn stop(&mut self) {
        self.runtime_context.stop();
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.runtime_context.set_animation_speed(speed);
    }

    pub fn canvas_size(&self) -> Option<(f32, f32)> {
        let name = self.current_animation.as_ref()?;
        let resource = self.resource.as_ref()?;
        let anim_resource = resource.resource()?;
        let idx = anim_resource.find_animation_index(name)?;
        let anim_meta = &anim_resource.animations()[idx];
        Some((anim_meta.canvas_width, anim_meta.canvas_height))
    }

    pub fn animation_names(&self) -> Option<Vec<String>> {
        let resource = self.resource.as_ref()?;
        let anim_resource = resource.resource()?;
        Some(anim_resource.get_animation_names())
    }
}
