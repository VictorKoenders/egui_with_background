use glium::glutin::{event::VirtualKeyCode, event_loop::EventLoopProxy};

pub trait Background {
    fn start_loading_image(&self, key: crate::image::Key, ctx: crate::image::LoadContext);
}

pub trait RepaintSignalMessage: Send + Sync + Sized + std::fmt::Debug + 'static {
    fn repaint_signal() -> Self;
    fn is_repaint_signal(&self) -> bool;
    fn is_image_loaded_response(&self) -> Option<crate::image::ToUIImage>;
}

pub trait App: 'static {
    type Background: Background;
    type Msg: RepaintSignalMessage;

    fn title(&self) -> &'static str;
    fn is_running(&self) -> bool;
    fn spawn_background(&self, proxy: EventLoopProxy<Self::Msg>) -> Self::Background;
    fn handle_message(&mut self, bg: &mut Self::Background, msg: Self::Msg);
    fn key_pressed(&mut self, bg: &mut Self::Background, key: VirtualKeyCode);
    fn key_released(&mut self, bg: &mut Self::Background, key: VirtualKeyCode);
    fn draw(&mut self, context: &mut crate::Context<Self::Background>);
}
