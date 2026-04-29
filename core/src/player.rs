
pub struct Player<'a> {
    pub runtime_context: ssruntime::Context<'a>
}
impl<'a> Player<'a> {
    pub fn new() -> Self {
        Self {
            runtime_context: ssruntime::Context::new()
        }
    }

    /*
    void setSsabResource( const Ref<GdSsabResource>& ssabRes );
    Ref<GdSsabResource> getSsabResource() const;
    void setAnimation( const String& strName );
    String getAnimation() const;
    */

    pub fn is_playing(&self) -> bool {
        self.runtime_context.is_playing()
    }
    pub fn play(&mut self, start_frame: f32) {
        if start_frame >= 0.0 {
            self.runtime_context.play_with_start_frame(start_frame)
        } else {
            self.runtime_context.play();
        }
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

    /*
    float getSpeed() const;
    void setFrame( float p_frame );
    float getFrame() const;

    int getTotalFrames() const;

    void setFrameRate( int p_fps );
    int getFrameRate() const;

    void setAnimationSection( int p_start, int p_end );
    int getAnimationSectionStart() const;
    int getAnimationSectionEnd() const;

    void setPlaybackDirection( int p_direction, int p_style );
    int getPlaybackDirection() const;
    int getPlaybackStyle() const;

    void setLoop( int p_count );
    int getLoop() const;

    void setSkipFrames( bool p_skip );
    bool isSkipFrames() const;

    void setSubFrameEnabled( bool p_enabled );
    bool isSubFrameEnabled() const;
    */
}

