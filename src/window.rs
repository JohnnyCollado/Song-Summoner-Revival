/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Abstraction of window setup, OpenGL context creation and event handling.
//!
//! Implemented using the sdl2 crate (a Rust wrapper for SDL2). All usage of
//! SDL should be confined to this module.
//!
//! There is currently no separation of concerns between a single window and
//! window system interaction in general, because it is assumed only one window
//! will be needed for the runtime of the app.

use crate::gles::present::{present_frame, FocusMarker, Overlay};
use crate::gles::{create_gles1_ctx_no_parent_stack, GLESContext, GLES};
use crate::image::Image;
use crate::matrix::Matrix;
use crate::options::Options;
use crate::Environment;
use sdl2::mouse::MouseButton;
use sdl2::pixels::PixelFormatEnum;
use sdl2::surface::Surface;
use sdl2_sys::SDL_PowerState;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::f32::consts::FRAC_PI_2;
use std::num::NonZeroU32;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set by the Android gear button (`MainActivity.nativeOpenSetupMenu`,
/// from the UI thread) and turned into [Event::OpenSetupMenu] at the next
/// poll.
static SETUP_MENU_REQUESTED: AtomicBool = AtomicBool::new(false);

/// JNI: the gear button over the game was tapped.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn Java_org_touchhle_android_MainActivity_nativeOpenSetupMenu(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
) {
    SETUP_MENU_REQUESTED.store(true, Ordering::Relaxed);
}

#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum DeviceFamily {
    iPhone,
    iPad,
}
impl std::fmt::Display for DeviceFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}
impl DeviceFamily {
    pub fn portrait_size(&self) -> (u32, u32) {
        match self {
            DeviceFamily::iPhone => (320, 480),
            DeviceFamily::iPad => (768, 1024),
        }
    }
}
impl TryFrom<u64> for DeviceFamily {
    type Error = ();
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(DeviceFamily::iPhone),
            2 => Ok(DeviceFamily::iPad),
            _ => Err(()),
        }
    }
}
impl TryFrom<&str> for DeviceFamily {
    type Error = ();
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "iphone" => Ok(DeviceFamily::iPhone),
            "ipad" => Ok(DeviceFamily::iPad),
            _ => Err(()),
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum DeviceOrientation {
    Portrait,
    LandscapeLeft,
    LandscapeRight,
}
fn size_for_orientation(
    family: DeviceFamily,
    orientation: DeviceOrientation,
    scale_hack: NonZeroU32,
) -> (u32, u32) {
    let (width, height) = family.portrait_size();
    let scale_hack = scale_hack.get();
    match orientation {
        DeviceOrientation::Portrait => (width * scale_hack, height * scale_hack),
        DeviceOrientation::LandscapeLeft => (height * scale_hack, width * scale_hack),
        DeviceOrientation::LandscapeRight => (height * scale_hack, width * scale_hack),
    }
}
fn rotate_fullscreen_size(orientation: DeviceOrientation, screen_size: (u32, u32)) -> (u32, u32) {
    let (short_side, long_side) = if screen_size.0 < screen_size.1 {
        (screen_size.0, screen_size.1)
    } else {
        (screen_size.1, screen_size.0)
    };
    match orientation {
        DeviceOrientation::Portrait => (short_side, long_side),
        DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight => {
            (long_side, short_side)
        }
    }
}
/// Tell SDL2 what orientation we want. Only useful on Android.
fn set_sdl2_orientation(orientation: DeviceOrientation) {
    // Despite the name, this hint works on Android too.
    sdl2::hint::set(
        "SDL_IOS_ORIENTATIONS",
        match orientation {
            DeviceOrientation::Portrait => "Portrait",
            // The inversion is deliberate. These probably correspond to
            // iPhone OS content orientations?
            DeviceOrientation::LandscapeLeft => "LandscapeRight",
            DeviceOrientation::LandscapeRight => "LandscapeLeft",
        },
    );
}

/// A point in the window's app viewport (`(x, y, width, height)`, as
/// [Window::viewport] gives) to the app's portrait screen space
/// (`screen_size`, in points), undoing the rotation. This is how input is
/// mapped.
fn viewport_to_screen(
    (in_x, in_y): (f32, f32),
    (vx, vy, vw, vh): (u32, u32, u32, u32),
    rotation: &Matrix<2>,
    (out_w, out_h): (u32, u32),
) -> (f32, f32) {
    // normalize to unit square centred on origin
    let x = (in_x - vx as f32) / vw as f32 - 0.5;
    let y = (in_y - vy as f32) / vh as f32 - 0.5;
    // rotate
    let [x, y] = rotation.inverse().unwrap().transform([x, y]);
    // back to pixels
    ((x + 0.5) * out_w as f32, (y + 0.5) * out_h as f32)
}

/// The inverse of [viewport_to_screen]: where a point of the app's screen
/// appears in the window. Used to draw over the app.
fn screen_to_viewport(
    (in_x, in_y): (f32, f32),
    (vx, vy, vw, vh): (u32, u32, u32, u32),
    rotation: &Matrix<2>,
    (screen_w, screen_h): (u32, u32),
) -> (f32, f32) {
    let x = in_x / screen_w as f32 - 0.5;
    let y = in_y / screen_h as f32 - 0.5;
    let [x, y] = rotation.transform([x, y]);
    (vx as f32 + (x + 0.5) * vw as f32, vy as f32 + (y + 0.5) * vh as f32)
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum FingerId {
    Mouse,
    Touch(i64),
    VirtualCursor,
    ButtonToTouch(crate::options::Button),
    StickToTouch,
    DpadToTouch,
}
pub type Coords = (f32, f32);

struct DpadState {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    active: bool,
}

/// A game controller button, by position. SDL's button labels are turned
/// off (see [Window::new]), so `FaceSouth` is the bottom face button on
/// every pad, whatever is printed on it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PadButton {
    FaceSouth,
    FaceEast,
    FaceWest,
    FaceNorth,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    LeftShoulder,
    RightShoulder,
    Start,
    Back,
    /// L2 / R2. SDL reports them as axes; [trigger_edge] turns them into
    /// presses.
    LeftTrigger,
    RightTrigger,
}

/// A trigger's axis (0 to 1) as a button: `Some(true)` when it goes past
/// half-way, `Some(false)` when it comes back below a quarter, `None`
/// otherwise. The gap between the two keeps a half-pulled trigger from
/// chattering.
pub fn trigger_edge(was_down: bool, value: f32) -> Option<bool> {
    if !was_down && value > 0.5 {
        Some(true)
    } else if was_down && value < 0.25 {
        Some(false)
    } else {
        None
    }
}

/// The D-pad direction the left stick counts as, given the one it counted
/// as before (`was`) and where it is now (-1 to 1 each way, y down). Like
/// the triggers: pushed past half-way presses the direction of whichever
/// axis is pushed further, and it stays pressed until it's back below a
/// quarter, so a stick resting part-way doesn't chatter.
pub fn stick_dpad(was: Option<PadButton>, x: f32, y: f32) -> Option<PadButton> {
    let (dominant, direction) = if x.abs() >= y.abs() {
        (
            x.abs(),
            if x < 0.0 {
                PadButton::DPadLeft
            } else {
                PadButton::DPadRight
            },
        )
    } else {
        (
            y.abs(),
            if y < 0.0 {
                PadButton::DPadUp
            } else {
                PadButton::DPadDown
            },
        )
    };
    if dominant > 0.5 {
        return Some(direction);
    }
    let along = |b: PadButton| match b {
        PadButton::DPadLeft => -x,
        PadButton::DPadRight => x,
        PadButton::DPadUp => -y,
        PadButton::DPadDown => y,
        _ => 0.0,
    };
    was.filter(|&b| along(b) >= 0.25)
}

/// The D-pad and the left stick as one set of directions: a direction is
/// down while either holds it, so holding both never presses it twice.
#[derive(Default, Debug)]
pub struct Directions {
    dpad: [bool; 4],
    stick: Option<PadButton>,
}

const DIRECTIONS: [PadButton; 4] = [
    PadButton::DPadUp,
    PadButton::DPadDown,
    PadButton::DPadLeft,
    PadButton::DPadRight,
];

impl Directions {
    fn index(b: PadButton) -> usize {
        DIRECTIONS.iter().position(|&d| d == b).unwrap()
    }
    fn down(&self, b: PadButton) -> bool {
        self.dpad[Self::index(b)] || self.stick == Some(b)
    }
    pub fn stick(&self) -> Option<PadButton> {
        self.stick
    }
    /// A D-pad button went down or up: the press or release to send, if
    /// the direction changed.
    pub fn set_dpad(&mut self, b: PadButton, pressed: bool) -> Option<(PadButton, bool)> {
        let before = self.down(b);
        self.dpad[Self::index(b)] = pressed;
        let after = self.down(b);
        (before != after).then_some((b, after))
    }
    /// The stick now points `dir`: the releases, then presses, to send.
    pub fn set_stick(&mut self, dir: Option<PadButton>) -> Vec<(PadButton, bool)> {
        let before = DIRECTIONS.map(|b| self.down(b));
        self.stick = dir;
        let after = DIRECTIONS.map(|b| self.down(b));
        let mut out = Vec::new();
        for pressed in [false, true] {
            for i in 0..4 {
                if before[i] != after[i] && after[i] == pressed {
                    out.push((DIRECTIONS[i], pressed));
                }
            }
        }
        out
    }
}

#[derive(Debug)]
pub enum TextInputEvent {
    Text(String),
    Backspace,
    Return,
}

#[derive(Debug)]
pub enum Event {
    /// User requested quit.
    Quit,
    /// OS has informed touchHLE it will soon become inactive.
    /// (iOS `applicationWillResignActive:`, Android `onPause()`)
    AppWillResignActive,
    /// OS has informed touchHLE it will soon terminate.
    /// (iOS `applicationWillTerminate:`, Android `onDestroy()`)
    AppWillTerminate,
    TouchesDown(HashMap<FingerId, Coords>),
    TouchesMove(HashMap<FingerId, Coords>),
    TouchesUp(HashMap<FingerId, Coords>),
    /// User pressed F12, requesting that execution be paused and the debugger
    /// take over.
    EnterDebugger,
    TextInput(TextInputEvent),
    /// A controller button went down or up, for menus touchHLE draws itself
    /// (the Song Summoner picker). Sent as well as, not instead of, any
    /// `--button-to-touch=` style touch the button is mapped to.
    ControllerButton {
        button: PadButton,
        pressed: bool,
        /// `SDL_GameControllerType` of the pad it came from, for picking
        /// which make's button icons to draw.
        controller_type: u32,
    },
    /// A key went down or up (desktop, or a keyboard connected to an
    /// Android device), by SDL scancode name ("W",
    /// "Return"). Held-key repeats aren't sent. F2, F11 and F12 aren't
    /// either: they're touchHLE's own.
    Key { key: String, pressed: bool },
    /// F2, or the Android gear button: open Song Summoner's Setup menu.
    OpenSetupMenu,
}

pub enum BatteryState {
    Unknown,
    OnBattery,
    NoBattery,
    Charging,
    Full,
}

pub enum GLVersion {
    /// OpenGL ES 1.1
    GLES11,
    /// OpenGL 2.1 compatibility profile
    GL21Compat,
}

pub struct GLContext(sdl2::video::GLContext);

impl GLContext {
    pub fn is_current(&self) -> bool {
        self.0.is_current()
    }
}

fn surface_from_image(image: &Image) -> Surface<'_> {
    let src_pixels = image.pixels();
    let (width, height) = image.dimensions();

    let mut surface = Surface::new(width, height, PixelFormatEnum::RGBA32).unwrap();
    let (width, height) = (width as usize, height as usize);
    let pitch = surface.pitch() as usize;
    surface.with_lock_mut(|dst_pixels| {
        for y in 0..height {
            for x in 0..width {
                for channel in 0..4 {
                    let src_idx = y * width * 4 + x * 4 + channel;
                    let dst_idx = y * pitch + x * 4 + channel;
                    dst_pixels[dst_idx] = src_pixels[src_idx];
                }
            }
        }
    });
    surface
}

pub struct Window {
    _sdl_ctx: sdl2::Sdl,
    video_ctx: sdl2::VideoSubsystem,
    window: sdl2::video::Window,
    event_pump: sdl2::EventPump,
    event_queue: VecDeque<Event>,
    last_polled: Instant,
    /// Separate queue for extremely high-priority events (e.g. app about to
    /// terminate).
    high_priority_event: Option<Event>,
    enable_event_polling: bool,
    #[cfg(target_os = "macos")]
    max_height: u32,
    #[cfg(target_os = "macos")]
    viewport_y_offset: u32,
    /// Copy of `fullscreen` on [Options]. Note that this is meaningless when
    /// [Self::rotatable_fullscreen] returns [true].
    fullscreen: bool,
    scale_hack: NonZeroU32,
    internal_gl_ins: Option<Box<dyn GLESContext>>,
    splash_image: Option<Image>,
    device_family: DeviceFamily,
    device_orientation: DeviceOrientation,
    controller_ctx: sdl2::GameControllerSubsystem,
    controllers: Vec<sdl2::controller::GameController>,
    dpad_state: DpadState,
    stick_active: bool,
    /// Whether L2 and R2 are pulled, as buttons (see [trigger_edge]).
    triggers_down: [bool; 2],
    /// The D-pad and the left stick, merged (see [Directions]).
    directions: Directions,
    /// Images drawn over the app, like Song Summoner's Setup menu (see
    /// [Self::set_overlays]).
    overlays: Vec<Overlay>,
    /// An app text field has the keyboard (see [Self::start_text_input]),
    /// so keys are typing, not controls.
    text_input_active: std::cell::Cell<bool>,
    _sensor_ctx: sdl2::SensorSubsystem,
    accelerometer: Option<sdl2::sensor::Sensor>,
    virtual_cursor_last: Option<(f32, f32, bool, bool)>,
    virtual_cursor_last_unsticky: Option<(f32, f32, Instant)>,
    /// A rectangle (x, y, width, height, in the app's portrait screen
    /// points) to outline over the app, and how: the controller's focus in
    /// menus that have none of their own (see [Self::set_focus_marker]).
    focus_marker: Option<FocusMarker>,
    /// Lines (from, to, in the app's portrait screen points) to draw over
    /// the app, for debugging (see [Self::set_debug_lines]).
    debug_lines: Vec<((f32, f32), (f32, f32))>,
    virtual_accelerometer_last: Option<(f32, f32, bool)>,
    /// Whether or not we are on the "main" environment stack (rather than
    /// a coroutine stack). Checked in various functions to make sure that
    /// certain SDL functions (that call JNI functions) are on the main
    /// stack on Android.
    pub(super) on_main_stack: bool,
}

impl Window {
    /// Returns [true] if touchHLE is running on a device where we should always
    /// display fullscreen, but SDL2 will let us control the orientation, i.e.
    /// Android devices.
    pub fn rotatable_fullscreen() -> bool {
        env::consts::OS == "android"
    }
    pub fn new(
        title: &str,
        icon: Option<Image>,
        launch_image: Option<Image>,
        options: &Options,
    ) -> Window {
        let sdl_ctx = sdl2::init().unwrap();
        let video_ctx = sdl_ctx.video().unwrap();

        // The "hidapi" feature of rust-sdl2 is enabled so that sdl2::sensor
        // is available, but we don't want to enable SDL's HIDAPI controller
        // drivers because they cause duplicated controllers on macOS
        // (https://github.com/libsdl-org/SDL/issues/7479). Once that's fixed,
        // remove this (https://github.com/touchHLE/touchHLE/issues/85).
        sdl2::hint::set("SDL_JOYSTICK_HIDAPI", "0");

        // Name buttons by position, not by the letter printed on them. By
        // default SDL calls a Switch pad's right button "A" because that's
        // its label, which puts it where an Xbox pad's B is. Positions keep
        // the confirm/back buttons and their on-screen icons consistent
        // across makes (see PadButton).
        sdl2::hint::set("SDL_GAMECONTROLLER_USE_BUTTON_LABELS", "0");

        if env::consts::OS == "android" {
            // It's important to set context version BEFORE window creation
            // ref. https://wiki.libsdl.org/SDL2/SDL_GLattr
            let attr = video_ctx.gl_attr();
            attr.set_context_version(1, 1);
            attr.set_context_profile(sdl2::video::GLProfile::GLES);

            // Disable blocking of event loop when app is paused.
            sdl2::hint::set("SDL_ANDROID_BLOCK_ON_PAUSE", "0");
        }

        // Separate mouse and touch events
        sdl2::hint::set("SDL_TOUCH_MOUSE_EVENTS", "0");

        // SDL2 disables the screen saver by default, but iPhone OS enables
        // the idle timer that triggers sleep by default, so we turn it back on
        // here, and then the app can disable it if it wants to.
        video_ctx.enable_screen_saver();

        let scale_hack = options.scale_hack;
        // TODO: some apps specify their orientation in Info.plist, we could use
        // that here.
        let device_family = options.device_family.unwrap_or(DeviceFamily::iPhone);
        let device_orientation = options.initial_orientation;
        let fullscreen = options.fullscreen;

        let mut window = if Self::rotatable_fullscreen() {
            // Without this, SDL will force fullscreen mode to be portrait.
            set_sdl2_orientation(device_orientation);
            let screen_size = video_ctx.display_bounds(0).unwrap().size();
            let (width, height) = rotate_fullscreen_size(device_orientation, screen_size);
            let window = video_ctx
                .window(title, width, height)
                .fullscreen()
                .opengl()
                .build()
                .unwrap();
            window
        } else if fullscreen {
            let (width, height) = video_ctx.display_bounds(0).unwrap().size();
            let window = video_ctx
                .window(title, width, height)
                .fullscreen_desktop()
                .opengl()
                .build()
                .unwrap();
            window
        } else {
            let (width, height) =
                size_for_orientation(device_family, device_orientation, scale_hack);
            let mut builder = video_ctx.window(title, width, height);
            builder.position_centered().resizable().opengl();
            // The output is letterboxed to fit, like a hand-resized window;
            // restoring it goes back to the usual size, centred.
            if options.maximized {
                builder.maximized();
            }
            builder.build().unwrap()
        };

        if env::consts::OS == "android" {
            // Sanity check
            let gl_attr = video_ctx.gl_attr();
            debug_assert_eq!(gl_attr.context_profile(), sdl2::video::GLProfile::GLES);
            debug_assert_eq!(gl_attr.context_version(), (1, 1));
        }

        if let Some(icon) = icon {
            window.set_icon(surface_from_image(&icon));
        }

        let event_pump = sdl_ctx.event_pump().unwrap();

        let controller_ctx = sdl_ctx.game_controller().unwrap();

        let sensor_ctx = sdl_ctx.sensor().unwrap();
        let mut accelerometer: Option<sdl2::sensor::Sensor> = None;
        if let Ok(num_sensors) = sensor_ctx.num_sensors() {
            for sensor_idx in 0..num_sensors {
                if let Ok(sensor) = sensor_ctx.open(sensor_idx) {
                    if sensor.sensor_type() == sdl2::sensor::SensorType::Accelerometer {
                        log!("Accelerometer detected: {}.", sensor.name());
                        accelerometer = Some(sensor);
                        break;
                    }
                }
            }
        }

        #[cfg(target_os = "macos")]
        let max_height = window.size().1;

        let mut window = Window {
            _sdl_ctx: sdl_ctx,
            video_ctx,
            window,
            event_pump,
            event_queue: VecDeque::new(),
            last_polled: Instant::now() - Duration::from_secs(1),
            high_priority_event: None,
            enable_event_polling: true,
            #[cfg(target_os = "macos")]
            max_height,
            #[cfg(target_os = "macos")]
            viewport_y_offset: 0,
            fullscreen,
            scale_hack,
            internal_gl_ins: None,
            splash_image: launch_image,
            device_family,
            device_orientation,
            controller_ctx,
            controllers: Vec::new(),
            dpad_state: DpadState {
                left: false,
                right: false,
                up: false,
                down: false,
                active: false,
            },
            stick_active: false,
            triggers_down: [false; 2],
            directions: Directions::default(),
            overlays: Vec::new(),
            text_input_active: std::cell::Cell::new(false),
            _sensor_ctx: sensor_ctx,
            accelerometer,
            virtual_cursor_last: None,
            virtual_cursor_last_unsticky: None,
            focus_marker: None,
            debug_lines: Vec::new(),
            virtual_accelerometer_last: None,
            on_main_stack: true,
        };

        // Set up OpenGL ES context used for splash screen and app UI rendering
        // (see src/frameworks/core_animation/composition.rs). OpenGL ES is used
        // because SDL2 won't let us use more than one graphics API in the same
        // window, and we also need OpenGL ES for the app's own rendering.
        let mut gl_ins = create_gles1_ctx_no_parent_stack(&mut window, options);
        {
            let gl_ctx = gl_ins.make_current(&mut window);
            log!("Driver info: {}", unsafe { gl_ctx.driver_description() });
        }
        window.internal_gl_ins = Some(gl_ins);

        if window.splash_image.is_some() {
            window.display_splash();
        }

        window
    }

    /// Poll for events from the OS. This needs to be done reasonably often
    /// (60Hz is probably fine) so that the host OS doesn't consider touchHLE
    /// to be unresponsive. Note that events are not returned by this function,
    /// since we often need to defer actually handling them.
    ///
    /// Since polling can be quite expensive, this function will skip it if it
    /// was called too recently.
    pub fn poll_for_events(&mut self, options: &Options) {
        assert!(self.on_main_stack);
        let now = Instant::now();
        // poll roughly twice per frame to try to avoid missing frames sometimes
        if now.duration_since(self.last_polled) < Duration::from_secs_f64(1.0 / 120.0) {
            return;
        }
        self.last_polled = now;

        fn transform_input_coords(
            window: &Window,
            (in_x, in_y): (f32, f32),
            independent_of_viewport: bool,
        ) -> (f32, f32) {
            let (vx, vy, vw, vh) = if independent_of_viewport {
                let (width, height) = size_for_orientation(
                    window.device_family,
                    window.device_orientation,
                    NonZeroU32::new(1).unwrap(),
                );
                (0, 0, width, height)
            } else {
                window.viewport()
            };
            let (out_x, out_y) = viewport_to_screen(
                (in_x, in_y),
                (vx, vy, vw, vh),
                &window.rotation_matrix(),
                window.size_unrotated_unscaled(),
            );
            // Round to match touch precision of official devices.
            (out_x.round(), out_y.round())
        }
        fn transform_virt_accel_coords(window: &Window, (in_x, in_y): (i32, i32)) -> (f32, f32) {
            let (_, _, vw, vh) = window.viewport();
            let out_x = ((in_x as f32 / vw as f32) * 2.0 - 1.0).clamp(-1.0, 1.0);
            let out_y = ((in_y as f32 / vh as f32) * 2.0 - 1.0).clamp(-1.0, 1.0);
            (out_x, out_y)
        }
        fn translate_button(button: sdl2::controller::Button) -> Option<crate::options::Button> {
            match button {
                sdl2::controller::Button::DPadLeft => Some(crate::options::Button::DPadLeft),
                sdl2::controller::Button::DPadUp => Some(crate::options::Button::DPadUp),
                sdl2::controller::Button::DPadRight => Some(crate::options::Button::DPadRight),
                sdl2::controller::Button::DPadDown => Some(crate::options::Button::DPadDown),
                sdl2::controller::Button::Start => Some(crate::options::Button::Start),
                sdl2::controller::Button::A => Some(crate::options::Button::A),
                sdl2::controller::Button::B => Some(crate::options::Button::B),
                sdl2::controller::Button::X => Some(crate::options::Button::X),
                sdl2::controller::Button::Y => Some(crate::options::Button::Y),
                sdl2::controller::Button::LeftShoulder => {
                    Some(crate::options::Button::LeftShoulder)
                }
                _ => None,
            }
        }
        fn translate_pad_button(button: sdl2::controller::Button) -> Option<PadButton> {
            use sdl2::controller::Button as B;
            Some(match button {
                B::A => PadButton::FaceSouth,
                B::B => PadButton::FaceEast,
                B::X => PadButton::FaceWest,
                B::Y => PadButton::FaceNorth,
                B::DPadUp => PadButton::DPadUp,
                B::DPadDown => PadButton::DPadDown,
                B::DPadLeft => PadButton::DPadLeft,
                B::DPadRight => PadButton::DPadRight,
                B::LeftShoulder => PadButton::LeftShoulder,
                B::RightShoulder => PadButton::RightShoulder,
                B::Start => PadButton::Start,
                B::Back => PadButton::Back,
                _ => return None,
            })
        }
        fn controller_type(instance_id: u32) -> u32 {
            // SAFETY: SDL returns null for an unknown id, which
            // SDL_GameControllerGetType treats as "unknown".
            unsafe {
                let controller = sdl2_sys::SDL_GameControllerFromInstanceID(instance_id as i32);
                sdl2_sys::SDL_GameControllerGetType(controller) as u32
            }
        }
        fn finger_absolute_coords(window: &Window, (x, y): (f32, f32)) -> (f32, f32) {
            let (screen_width, screen_height) = window.window.drawable_size();
            (screen_width as f32 * x, screen_height as f32 * y)
        }

        if SETUP_MENU_REQUESTED.swap(false, Ordering::Relaxed) {
            self.event_queue.push_back(Event::OpenSetupMenu);
        }

        let mut controller_updated = false;
        // event_pump doesn't have a method to peek on events
        // so, we keep track of an unconsumed one from a previous loop iteration
        // FIXME: use peek_event() from even_subsystem
        let mut previous_event: Option<sdl2::event::Event> = None;
        while self.enable_event_polling {
            use sdl2::event::Event as E;
            let event = if let Some(e) = previous_event.take() {
                match e {
                    E::Unknown { .. } => (),
                    _ => log_dbg!("Consuming previous event: {:?}", e),
                }
                e
            } else if let Some(e) = self.event_pump.poll_event() {
                match e {
                    E::Unknown { .. } => (),
                    _ => log_dbg!("Consuming new event: {:?}", e),
                }
                e
            } else {
                break;
            };

            // Virtual accelerometer
            match event {
                E::MouseButtonDown {
                    x,
                    y,
                    mouse_btn: MouseButton::Right,
                    ..
                } => {
                    let (x, y) = transform_virt_accel_coords(self, (x, y));
                    self.virtual_accelerometer_last = Some((x, y, true));
                }
                E::MouseMotion {
                    x, y, mousestate, ..
                } if mousestate.right() => {
                    let (x, y) = transform_virt_accel_coords(self, (x, y));
                    self.virtual_accelerometer_last = Some((x, y, true));
                }
                E::MouseButtonUp {
                    x,
                    y,
                    mouse_btn: MouseButton::Right,
                    ..
                } => {
                    let (x, y) = transform_virt_accel_coords(self, (x, y));
                    self.virtual_accelerometer_last = Some((x, y, false));
                }
                _ => {}
            }

            // Controller buttons for touchHLE's own menus. The touch
            // mappings below still see the same event.
            match event {
                E::ControllerButtonDown { which, button, .. }
                | E::ControllerButtonUp { which, button, .. } => {
                    if let Some(button) = translate_pad_button(button) {
                        let pressed = matches!(event, E::ControllerButtonDown { .. });
                        // Directions go through the merge with the stick.
                        let change = if DIRECTIONS.contains(&button) {
                            self.directions.set_dpad(button, pressed)
                        } else {
                            Some((button, pressed))
                        };
                        if let Some((button, pressed)) = change {
                            self.event_queue.push_back(Event::ControllerButton {
                                button,
                                pressed,
                                controller_type: controller_type(which),
                            });
                        }
                    }
                }
                // The left stick mirrors the D-pad in touchHLE's menus.
                E::ControllerAxisMotion { which, axis, .. }
                    if matches!(
                        axis,
                        sdl2::controller::Axis::LeftX | sdl2::controller::Axis::LeftY
                    ) =>
                {
                    let (x, y, _) = self.get_controller_stick(options, true);
                    let dir = stick_dpad(self.directions.stick(), x, y);
                    for (button, pressed) in self.directions.set_stick(dir) {
                        self.event_queue.push_back(Event::ControllerButton {
                            button,
                            pressed,
                            controller_type: controller_type(which),
                        });
                    }
                }
                // Keys for Song Summoner's controls and Setup menu, on
                // desktops and from a Bluetooth or USB keyboard on Android
                // (SDL sends pads' buttons as controller events, not
                // keys). The text-input handling below still sees the
                // same event.
                E::KeyDown {
                    scancode: Some(scancode),
                    repeat: false,
                    ..
                }
                | E::KeyUp {
                    scancode: Some(scancode),
                    repeat: false,
                    ..
                } => {
                    use sdl2::keyboard::Scancode;
                    let pressed = matches!(event, E::KeyDown { .. });
                    match scancode {
                        Scancode::F2 => {
                            if pressed {
                                self.event_queue.push_back(Event::OpenSetupMenu);
                            }
                        }
                        Scancode::F11 | Scancode::F12 => (),
                        _ => self.event_queue.push_back(Event::Key {
                            key: scancode.name().to_string(),
                            pressed,
                        }),
                    }
                }
                // The triggers are axes; touchHLE's menus want them as
                // buttons.
                E::ControllerAxisMotion {
                    which, axis, value, ..
                } if matches!(
                    axis,
                    sdl2::controller::Axis::TriggerLeft | sdl2::controller::Axis::TriggerRight
                ) =>
                {
                    let (i, button) = if axis == sdl2::controller::Axis::TriggerLeft {
                        (0, PadButton::LeftTrigger)
                    } else {
                        (1, PadButton::RightTrigger)
                    };
                    let value = f32::from(value) / f32::from(i16::MAX);
                    if let Some(pressed) = trigger_edge(self.triggers_down[i], value) {
                        self.triggers_down[i] = pressed;
                        self.event_queue.push_back(Event::ControllerButton {
                            button,
                            pressed,
                            controller_type: controller_type(which),
                        });
                    }
                }
                _ => {}
            }

            self.event_queue.push_back(match event {
                E::Quit { .. } => Event::Quit,
                E::MouseButtonDown {
                    x,
                    y,
                    mouse_btn: MouseButton::Left,
                    ..
                } => {
                    let coords = transform_input_coords(self, (x as f32, y as f32), false);
                    log_dbg!("MouseButtonDown x {}, y {}, coords {:?}", x, y, coords);
                    Event::TouchesDown(HashMap::from([(FingerId::Mouse, coords)]))
                }
                E::MouseMotion {
                    x, y, mousestate, ..
                } if mousestate.left() => {
                    let coords = transform_input_coords(self, (x as f32, y as f32), false);
                    log_dbg!("MouseMotion x {}, y {}, coords {:?}", x, y, coords);
                    Event::TouchesMove(HashMap::from([(FingerId::Mouse, coords)]))
                }
                E::MouseButtonUp {
                    x,
                    y,
                    mouse_btn: MouseButton::Left,
                    ..
                } => {
                    let coords = transform_input_coords(self, (x as f32, y as f32), false);
                    log_dbg!("MouseButtonUp x {}, y {}, coords {:?}", x, y, coords);
                    Event::TouchesUp(HashMap::from([(FingerId::Mouse, coords)]))
                }
                E::ControllerDeviceAdded { which, .. } => {
                    self.controller_added(which);
                    continue;
                }
                E::ControllerDeviceRemoved { which, .. } => {
                    self.controller_removed(which);
                    continue;
                }
                // Note that accelerometer simulation with analog sticks is
                // handled with polling, rather than being event-based.
                E::ControllerButtonUp { button, .. } | E::ControllerButtonDown { button, .. } => {
                    controller_updated = true;
                    let Some(button) = translate_button(button) else {
                        continue;
                    };
                    // Called whenever a DPad direction is pressed or released
                    if (button == crate::options::Button::DPadLeft
                        || button == crate::options::Button::DPadUp
                        || button == crate::options::Button::DPadRight
                        || button == crate::options::Button::DPadDown)
                        && options.dpad_to_touch.is_some()
                    {
                        let Some((x, y, w, h)) = options.dpad_to_touch else {
                            unreachable!();
                        };

                        // Update held state
                        let pressed = matches!(event, E::ControllerButtonDown { .. });
                        match button {
                            crate::options::Button::DPadLeft => self.dpad_state.left = pressed,
                            crate::options::Button::DPadRight => self.dpad_state.right = pressed,
                            crate::options::Button::DPadUp => self.dpad_state.up = pressed,
                            crate::options::Button::DPadDown => self.dpad_state.down = pressed,
                            _ => unreachable!(),
                        }

                        // Compute center
                        let cx = x + w * 0.5;
                        let cy = y + h * 0.5;

                        // Compute combined delta
                        let mut dx = 0.0;
                        let mut dy = 0.0;

                        if self.dpad_state.left {
                            dx -= 0.5 * w;
                        }
                        if self.dpad_state.right {
                            dx += 0.5 * w;
                        }
                        if self.dpad_state.up {
                            dy -= 0.5 * h;
                        }
                        if self.dpad_state.down {
                            dy += 0.5 * h;
                        }

                        // Final coords: center + movement
                        let coords = transform_input_coords(self, (cx + dx, cy + dy), true);

                        // Send TouchDown if any dpad is held, TouchUp if none
                        let any_held = self.dpad_state.left
                            || self.dpad_state.right
                            || self.dpad_state.up
                            || self.dpad_state.down;

                        if !self.dpad_state.active && any_held {
                            // New touch
                            self.dpad_state.active = true;
                            Event::TouchesDown(HashMap::from([(FingerId::DpadToTouch, coords)]))
                        } else if self.dpad_state.active && any_held {
                            // Move existing touch
                            Event::TouchesMove(HashMap::from([(FingerId::DpadToTouch, coords)]))
                        } else if self.dpad_state.active && !any_held {
                            // Release touch
                            self.dpad_state.active = false;
                            Event::TouchesUp(HashMap::from([(FingerId::DpadToTouch, coords)]))
                        } else {
                            continue;
                        }
                    } else {
                        let Some(&(x, y)) = options.button_to_touch.get(&button) else {
                            continue;
                        };
                        match event {
                            E::ControllerButtonUp { .. } => {
                                let coords = transform_input_coords(self, (x, y), true);
                                Event::TouchesUp(HashMap::from([(
                                    FingerId::ButtonToTouch(button),
                                    coords,
                                )]))
                            }
                            E::ControllerButtonDown { .. } => {
                                let coords = transform_input_coords(self, (x, y), true);
                                Event::TouchesDown(HashMap::from([(
                                    FingerId::ButtonToTouch(button),
                                    coords,
                                )]))
                            }
                            _ => unreachable!(),
                        }
                    }
                }
                E::ControllerAxisMotion { axis, .. } => {
                    controller_updated = true;
                    let Some((x, y, w, h)) = options.stick_to_touch else {
                        continue;
                    };
                    if axis == sdl2::controller::Axis::LeftX
                        || axis == sdl2::controller::Axis::LeftY
                    {
                        let (stick_x, stick_y, _) = self.get_controller_stick(options, true);
                        let coords = transform_input_coords(
                            self,
                            (
                                x + ((stick_x + 1.0) / 2.0) * w,
                                y + ((stick_y + 1.0) / 2.0) * h,
                            ),
                            true,
                        );
                        if stick_x.abs() < options.deadzone && stick_y.abs() < options.deadzone {
                            if !self.stick_active {
                                // Ignore deadzone events when stick is inactive
                                continue;
                            } else {
                                // Release touch when stick returns to deadzone
                                self.stick_active = false;
                                Event::TouchesUp(HashMap::from([(FingerId::StickToTouch, coords)]))
                            }
                        } else if !self.stick_active {
                            // New touch
                            self.stick_active = true;
                            Event::TouchesDown(HashMap::from([(FingerId::StickToTouch, coords)]))
                        } else {
                            // Move existing touch
                            Event::TouchesMove(HashMap::from([(FingerId::StickToTouch, coords)]))
                        }
                    } else {
                        continue;
                    }
                }
                E::AppWillEnterBackground { .. } => {
                    log!("Received app-will-resign-active event.");
                    assert!(self.high_priority_event.is_none());
                    self.high_priority_event = Some(Event::AppWillResignActive);
                    // For some reason, if we don't pause event polling, we will
                    // never finish handling the event.
                    // TODO: Add a mechanism for re-enabling polling, if at some
                    // point we support returning touchHLE to the foreground.
                    self.enable_event_polling = false;
                    continue;
                }
                E::AppTerminating { .. } => {
                    log!("Received app-will-terminate event.");
                    assert!(self.high_priority_event.is_none());
                    self.high_priority_event = Some(Event::AppWillTerminate);
                    self.enable_event_polling = false;
                    continue;
                }
                E::FingerUp {
                    timestamp,
                    finger_id,
                    x,
                    y,
                    ..
                }
                | E::FingerMotion {
                    timestamp,
                    finger_id,
                    x,
                    y,
                    ..
                }
                | E::FingerDown {
                    timestamp,
                    finger_id,
                    x,
                    y,
                    ..
                } => {
                    log_dbg!("Starting multi-touch for {:?}", event);
                    // To implement multi-touch we accumulate here same touch
                    // events at the same timestamp. This is consistent with
                    // UIKit, but could be broken if events come out of order.
                    // (in worst case we separate multi-touches in several ones)
                    // TODO: handle out of order touches
                    let curr_timestamp = timestamp;
                    let abs_coords = finger_absolute_coords(self, (x, y));
                    let coords = transform_input_coords(self, abs_coords, false);
                    log_dbg!("Finger event x {}, y {}, coords {:?}", x, y, coords);
                    let mut map = HashMap::from([(FingerId::Touch(finger_id), coords)]);
                    while let Some(next) = self.event_pump.poll_event() {
                        match next {
                            E::Unknown { .. } => (),
                            _ => log_dbg!("Next possible multi-touch event: {:?}", next),
                        }
                        match next {
                            E::FingerUp {
                                timestamp,
                                finger_id,
                                x,
                                y,
                                ..
                            }
                            | E::FingerMotion {
                                timestamp,
                                finger_id,
                                x,
                                y,
                                ..
                            }
                            | E::FingerDown {
                                timestamp,
                                finger_id,
                                x,
                                y,
                                ..
                            } if timestamp == curr_timestamp && next.is_same_kind_as(&event) => {
                                let abs_coords = finger_absolute_coords(self, (x, y));
                                let coords = transform_input_coords(self, abs_coords, false);
                                map.insert(FingerId::Touch(finger_id), coords);
                            }
                            E::MultiGesture { timestamp, .. } if timestamp == curr_timestamp => {
                                // TODO: handle gestures
                                continue;
                            }
                            _ => {
                                // event_pump doesn't have a method to peek on
                                // events, so we keep track of an unconsumed
                                // one from a previous loop iteration
                                assert!(previous_event.is_none());
                                previous_event = Some(next);
                                break;
                            }
                        }
                    }
                    log_dbg!("Finishing multi-touch for {:?} with {:?}", event, map);
                    match event {
                        E::FingerUp { .. } => Event::TouchesUp(map),
                        E::FingerMotion { .. } => Event::TouchesMove(map),
                        E::FingerDown { .. } => Event::TouchesDown(map),
                        _ => unreachable!(),
                    }
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::F12),
                    ..
                } => {
                    // Log this so you can tell when touchHLE has received
                    // the event but it's stuck in the queue.
                    echo!("F12 pressed, EnterDebugger event queued.");
                    Event::EnterDebugger
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::F11),
                    repeat: false,
                    ..
                } => {
                    // Toggle borderless fullscreen on platforms where it
                    // matters. Android (`rotatable_fullscreen`) is always
                    // fullscreen at the OS level so this is a no-op there.
                    // We use Desktop (borderless, same resolution) rather
                    // than True (mode-switching) — quicker toggle, no
                    // resolution flicker.
                    self.toggle_fullscreen();
                    continue;
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::Backspace),
                    ..
                } => {
                    log_dbg!("SDL TextInput Backspace");
                    Event::TextInput(TextInputEvent::Backspace)
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::Return),
                    ..
                } => {
                    log_dbg!("SDL TextInput Return");
                    Event::TextInput(TextInputEvent::Return)
                }
                E::TextInput { text, .. } => {
                    log_dbg!("SDL TextInput {}", text);
                    Event::TextInput(TextInputEvent::Text(text))
                }
                _ => continue,
            })
        }

        // With the cursor disabled, update_virtual_cursor never runs, so
        // virtual_cursor_last stays None and nothing is drawn either.
        if controller_updated && options.virtual_cursor {
            let (new_x, new_y, pressed, pressed_changed, moved) =
                self.update_virtual_cursor(options);
            self.event_queue
                .push_back(match (pressed, pressed_changed, moved) {
                    (true, true, _) => {
                        let coords = transform_input_coords(self, (new_x, new_y), false);
                        Event::TouchesDown(HashMap::from([(FingerId::VirtualCursor, coords)]))
                    }
                    (false, true, _) => {
                        let coords = transform_input_coords(self, (new_x, new_y), false);
                        Event::TouchesUp(HashMap::from([(FingerId::VirtualCursor, coords)]))
                    }
                    (true, _, true) => {
                        let coords = transform_input_coords(self, (new_x, new_y), false);
                        Event::TouchesMove(HashMap::from([(FingerId::VirtualCursor, coords)]))
                    }
                    _ => return,
                });
        }
    }

    /// Pop an event from the queue (in FIFO order, except for high priority
    /// events)
    pub fn pop_event(&mut self) -> Option<Event> {
        self.high_priority_event
            .take()
            .or_else(|| self.event_queue.pop_front())
    }

    fn controller_added(&mut self, joystick_idx: u32) {
        let Ok(controller) = self.controller_ctx.open(joystick_idx) else {
            log!("Warning: A new controller was connected, but it couldn't be accessed!");
            return;
        };

        let controller_name = controller.name();
        if env::consts::OS == "android" && controller_name.starts_with("uinput-") {
            log!("ignoring fingerprint device: {}", controller_name);
            return;
        }
        log!(
            "New controller connected: {}. Left stick = device tilt. Right stick = touch input (press the stick or shoulder button to tap/hold).",
            controller_name
        );
        self.controllers.push(controller);
    }
    fn controller_removed(&mut self, instance_id: u32) {
        let Some(idx) = self
            .controllers
            .iter()
            .position(|controller| controller.instance_id() == instance_id)
        else {
            return;
        };
        let controller = self.controllers.remove(idx);
        log!("Warning: Controller disconnected: {}", controller.name());
    }
    pub fn print_accelerometer_notice(&self, options: &Options) {
        log!("This app uses the accelerometer.");

        if !self.controllers.is_empty() && options.analog_stick_tilt_controls {
            log!("Your connected controller's left analog stick will be used for accelerometer simulation.");
            if self.accelerometer.is_some() {
                log!("Disconnect the controller if you want to use your device's accelerometer.");
            }
        } else if self.accelerometer.is_some() {
            log!("Your device's accelerometer will be used for accelerometer simulation.");
            if options.analog_stick_tilt_controls {
                log!("Connect a controller if you would prefer to use an analog stick.");
            }
        } else if self.controllers.is_empty() && options.analog_stick_tilt_controls {
            log!("Connect a controller to get accelerometer simulation.");
        }

        if self.accelerometer.is_none() {
            log!(
                "You can {}hold right click and move the cursor to simulate the accelerometer.",
                if options.analog_stick_tilt_controls {
                    "also "
                } else {
                    ""
                }
            );
        }
    }

    /// Get the real or simulated accelerometer output.
    /// See also [crate::frameworks::uikit::ui_accelerometer].
    pub fn get_acceleration(&self, options: &Options) -> (f32, f32, f32) {
        if self.controllers.is_empty() || !options.analog_stick_tilt_controls {
            if let Some(ref accelerometer) = self.accelerometer {
                let data = accelerometer.get_data().unwrap();
                let sdl2::sensor::SensorData::Accel(data) = data else {
                    panic!();
                };
                let [x, y, z] = data;
                // UIAcceleration reports acceleration towards gravity, but SDL2
                // reports acceleration away from gravity.
                let (x, y, z) = (-x, -y, -z);
                // UIAcceleration reports acceleration in units of g-force, but
                // SDL2 reports acceleration in units of m/s^2.
                let gravity: f32 = 9.80665; // SDL_STANDARD_GRAVITY
                let (x, y, z) = (x / gravity, y / gravity, z / gravity);
                return (x, y, z);
            }
        }

        let (x, y) = if self
            .virtual_accelerometer_last
            .is_some_and(|(_x, _y, right_click_hold)| right_click_hold)
        {
            self.virtual_accelerometer_last
                .map(|(x, y, _right_click_hold)| (x, y))
                .unwrap()
        } else {
            // Get left analog stick input. The range is [-1, 1] on each axis.
            let (x, y, _) = self.get_controller_stick(options, true);
            (x, y)
        };

        // Correct for window rotation
        let [x, y] = self.rotation_matrix().inverse().unwrap().transform([x, y]);
        let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0)); // just in case

        // Let's simulate tilting the device based on the analog stick inputs.
        //
        // If an iPhone is lying flat on its back, level with the ground, and it
        // is on Earth, the accelerometer will report approximately (0, 0, -1).
        // The acceleration x and y axes are aligned with the screen's x and y
        // axes. +x points to the right of the screen, +y points to the top of
        // the screen, and +z points away from the screen. In the example
        // scenario, the z axis is parallel to gravity.

        let gravity: [f32; 3] = [0.0, 0.0, -1.0];

        let neutral_x = options.x_tilt_offset.to_radians();
        let neutral_y = options.y_tilt_offset.to_radians();
        let x_rotation_range = options.x_tilt_range.to_radians() / 2.0;
        let y_rotation_range = options.y_tilt_range.to_radians() / 2.0;
        // (x, y) are swapped because the controller Y axis usually corresponds
        // to forward/backward movement, but rotating about the Y axis means
        // tilting the device left/right.
        let x_rotation = neutral_x - x_rotation_range * y;
        let y_rotation = neutral_y - y_rotation_range * x;
        let matrix =
            Matrix::<3>::y_rotation(y_rotation).multiply(&Matrix::<3>::x_rotation(x_rotation));
        let [x, y, z] = matrix.transform(gravity);

        (x, y, z)
    }

    /// Outline a rectangle of the app's screen (x, y, width, height, in
    /// portrait screen points, as `-[UIView convertRect:toView:nil]` gives)
    /// in the given shape, or stop. It stays until changed.
    pub fn set_focus_marker(&mut self, marker: Option<FocusMarker>) {
        self.focus_marker = marker;
    }

    /// For use when redrawing the screen: where the focus marker is in the
    /// window (x, y, width, height, in the same pixels as
    /// [Self::virtual_cursor_visible_at]) and its shape, if there is one.
    /// A diamond inscribed in the rectangle stays inscribed in it through
    /// the quarter-turn rotations.
    pub fn focus_marker_visible_at(&self) -> Option<FocusMarker> {
        let ((x, y, w, h), shape) = self.focus_marker?;
        let to_window = |p| {
            screen_to_viewport(
                p,
                self.viewport(),
                &self.rotation_matrix(),
                self.size_unrotated_unscaled(),
            )
        };
        // Rotations are by quarter turns, so opposite corners are enough.
        let (x0, y0) = to_window((x, y));
        let (x1, y1) = to_window((x + w, y + h));
        Some((
            (x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs()),
            shape,
        ))
    }

    /// Toggle borderless fullscreen (F11, or Song Summoner's Setup menu).
    /// Does nothing on Android, which is always fullscreen.
    pub fn toggle_fullscreen(&mut self) {
        if Self::rotatable_fullscreen() {
            return;
        }
        let new_state = if self.fullscreen {
            sdl2::video::FullscreenType::Off
        } else {
            sdl2::video::FullscreenType::Desktop
        };
        if let Err(e) = self.window.set_fullscreen(new_state) {
            log!("Couldn't toggle fullscreen: {}", e);
        } else {
            self.fullscreen = !self.fullscreen;
            echo!(
                "Window is now {}.",
                if self.fullscreen {
                    "fullscreen"
                } else {
                    "windowed"
                }
            );
        }
    }

    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// Draw images over the app, each stretched over its rectangle of the
    /// app's (landscape, as shown) screen, in fractions of it. They stay
    /// until changed; an empty list stops.
    pub fn set_overlays(&mut self, overlays: Vec<Overlay>) {
        self.overlays = overlays;
    }

    /// For use when redrawing the screen.
    pub fn overlays(&self) -> Vec<Overlay> {
        self.overlays.clone()
    }

    /// Where a point of the app's portrait screen (as touch events give
    /// them) is on the app's screen as shown, in fractions of it (0 to 1,
    /// from the top left).
    pub fn screen_point_to_shown_fraction(&self, point: (f32, f32)) -> (f32, f32) {
        let viewport = self.viewport();
        let (x, y) = screen_to_viewport(
            point,
            viewport,
            &self.rotation_matrix(),
            self.size_unrotated_unscaled(),
        );
        let (vx, vy, vw, vh) = viewport;
        (
            (x - vx as f32) / vw.max(1) as f32,
            (y - vy as f32) / vh.max(1) as f32,
        )
    }

    /// Whether an app text field has the keyboard.
    pub fn is_text_input_active(&self) -> bool {
        self.text_input_active.get()
    }

    /// Draw lines over the app (from, to, in portrait screen points, like
    /// [Self::set_focus_marker]), for debugging. They stay until changed;
    /// an empty list stops.
    pub fn set_debug_lines(&mut self, lines: Vec<((f32, f32), (f32, f32))>) {
        self.debug_lines = lines;
    }

    /// For use when redrawing the screen: the debug lines in the window, in
    /// the same pixels as [Self::focus_marker_visible_at].
    pub fn debug_lines_visible_at(&self) -> Vec<((f32, f32), (f32, f32))> {
        if self.debug_lines.is_empty() {
            return Vec::new();
        }
        let viewport = self.viewport();
        let rotation = self.rotation_matrix();
        let size = self.size_unrotated_unscaled();
        let to_window = |p| screen_to_viewport(p, viewport, &rotation, size);
        self.debug_lines
            .iter()
            .map(|&(from, to)| (to_window(from), to_window(to)))
            .collect()
    }

    /// For use when redrawing the screen: Get the cached on-screen position and
    /// press state of the analog stick-controlled virtual cursor, if it is
    /// visible.
    pub fn virtual_cursor_visible_at(&self) -> Option<(f32, f32, bool)> {
        let (x, y, pressed, visible) = self.virtual_cursor_last?;
        if visible {
            // When stickyness is in use, the visual cursor movement appears
            // uncomfortably choppy. Showing the un-sticky position is a bit
            // misleading but it *feels* better, and it is documented.
            if let Some((x_unsticky, y_unsticky, _time)) = self.virtual_cursor_last_unsticky {
                Some((x_unsticky, y_unsticky, pressed))
            } else {
                Some((x, y, pressed))
            }
        } else {
            None
        }
    }

    /// Update the virtual cursor's position, click state and visibility, then
    /// return the new position, pressed state, whether the press state changed
    /// and whether the cursor moved.
    fn update_virtual_cursor(&mut self, options: &Options) -> (f32, f32, bool, bool, bool) {
        // Get right analog stick input. The range is [-1, 1] on each axis.
        let (x, y, pressed) = self.get_controller_stick(options, false);

        // The cursor is intended to only show up once you move the analog stick
        // out of its deadzone, or while the button is held.
        let visible = pressed || x != 0.0 || y != 0.0;

        // Though the analog stick output fits within a square, its actual range
        // is usually a circle enclosed by the square. So we need to cut out the
        // rectangular shape of the screen from that circle within the square.
        let (vx, vy, vw, vh) = self.viewport();
        let (vx, vy, vw, vh) = (vx as f32, vy as f32, vw as f32, vh as f32);

        let (x, y) = {
            // Use Pythagoras's theorem to find the largest size the rectangle
            // can have within the circle.
            let ratio = vw / vh;
            let rect_height = (ratio * ratio + 1.0).powf(-0.5);
            let rect_width = ratio * rect_height;

            let x_abs = x.abs().min(rect_width) / rect_width;
            let y_abs = y.abs().min(rect_height) / rect_height;
            (x_abs.copysign(x), y_abs.copysign(y))
        };

        // Convert to on-screen window co-ordinates
        let x = (x / 2.0 + 0.5) * vw + vx;
        let y = (y / 2.0 + 0.5) * vh + vy;

        let (old_x, old_y, old_pressed, _old_visible) =
            self.virtual_cursor_last.unwrap_or_default();

        let (x, y) = if let Some((smoothing_strength, sticky_radius)) =
            options.stabilize_virtual_cursor
        {
            let new_time = Instant::now();

            let (old_x_unsticky, old_y_unsticky, old_time) = self
                .virtual_cursor_last_unsticky
                .unwrap_or((0.0, 0.0, new_time));

            let delta_t = new_time.saturating_duration_since(old_time).as_secs_f32();

            // Apply a feedback-based smoothing with exponential decay, to try
            // to dampen shakiness in the stick movement.

            let smooth = |old: f32, new: f32| -> f32 {
                if smoothing_strength != 0.0 {
                    let lerp_factor = 1.0 - (0.5_f32).powf(delta_t * (1.0 / smoothing_strength));
                    old + (new - old) * lerp_factor
                } else {
                    new
                }
            };

            let new_x_unsticky = smooth(old_x_unsticky, x);
            let new_y_unsticky = smooth(old_y_unsticky, y);

            self.virtual_cursor_last_unsticky = Some((new_x_unsticky, new_y_unsticky, new_time));

            // Make the reported position "sticky" within a certain radius, i.e.
            // if the new position's distance from the old one is within the
            // radius, report no change in position.

            if (new_x_unsticky - old_x).hypot(new_y_unsticky - old_y) < sticky_radius {
                (old_x, old_y)
            } else {
                (new_x_unsticky, new_y_unsticky)
            }
        } else {
            (x, y)
        };

        self.virtual_cursor_last = Some((x, y, pressed, visible));

        (
            x,
            y,
            pressed,
            pressed != old_pressed,
            x != old_x || y != old_y,
        )
    }

    /// Get the summed X and Y positions and button state of the left or right
    /// analog stick of the game controllers. Each axis value is in the range
    /// [-1, 1].
    fn get_controller_stick(&self, options: &Options, left: bool) -> (f32, f32, bool) {
        fn convert_axis(axis: i16, deadzone: f32) -> f32 {
            assert!(deadzone >= 0.0);
            let axis = ((axis as f32) / (i16::MAX as f32)).clamp(-1.0, 1.0);
            let abs_axis = (axis.abs().max(deadzone) - deadzone) / (1.0 - deadzone);
            abs_axis.copysign(axis)
        }

        let (mut x, mut y) = (0.0, 0.0);
        let mut pressed = false;
        for controller in &self.controllers {
            use sdl2::controller::{Axis, Button};
            let (x_axis, y_axis, button1, button2) = if left {
                (
                    Axis::LeftX,
                    Axis::LeftY,
                    Button::LeftStick,
                    Button::LeftShoulder,
                )
            } else {
                (
                    Axis::RightX,
                    Axis::RightY,
                    Button::RightStick,
                    Button::RightShoulder,
                )
            };
            x += convert_axis(controller.axis(x_axis), options.deadzone);
            y += convert_axis(controller.axis(y_axis), options.deadzone);
            pressed |= controller.button(button1);
            pressed |= controller.button(button2);
        }
        let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));

        (x, y, pressed)
    }

    pub fn create_gl_context(&self, version: GLVersion) -> Result<GLContext, String> {
        let attr = self.video_ctx.gl_attr();
        match version {
            GLVersion::GLES11 => {
                attr.set_context_version(1, 1);
                attr.set_context_profile(sdl2::video::GLProfile::GLES);
            }
            GLVersion::GL21Compat => {
                attr.set_context_version(2, 1);
                attr.set_context_profile(sdl2::video::GLProfile::Compatibility);
            }
        }

        let gl_ctx = self.window.gl_create_context()?;

        Ok(GLContext(gl_ctx))
    }

    pub fn gl_get_proc_address(&self, procname: &str) -> *const std::ffi::c_void {
        // For some reason, rust-sdl2 uses *const (), but () is not meant to be
        // used for void pointees (just void results), so let's fix that.
        self.video_ctx.gl_get_proc_address(procname) as *const _
    }

    pub fn set_share_with_current_context(&self, value: bool) {
        self.video_ctx
            .gl_attr()
            .set_share_with_current_context(value)
    }

    pub unsafe fn make_gl_context_current(&self, gl_ctx: &GLContext) {
        self.window.gl_make_current(&gl_ctx.0).unwrap();
    }

    /// Make the internal OpenGL ES context (for splash screen and UI rendering)
    /// current.
    #[must_use]
    pub fn make_internal_gl_ctx_current<'win>(&'win mut self) -> Box<dyn GLES + 'win> {
        // The invariant is held up here - since the instance we return is
        // bound to the lifetime of window, it can't outlive the internal GL
        // context and can't outlive the window.
        let gl_ins = unsafe {
            self.internal_gl_ins
                .as_mut()
                .unwrap()
                .make_current_unchecked_for_window(
                    &mut |gl_ctx| self.window.gl_make_current(&gl_ctx.0).unwrap(),
                    &mut |s| self.video_ctx.gl_get_proc_address(s) as *const _,
                )
        };
        gl_ins
    }

    fn display_splash(&mut self) {
        assert!(self.splash_image.is_some());

        // OpenGL ES expects bottom-to-top row order for image data, but our
        // image data will be top-to-bottom. A reflection transform compensates.
        let matrix = self.rotation_matrix().multiply(&Matrix::y_flip());
        let (vx, vy, vw, vh) = self.viewport();
        let viewport = (vx, vy + self.viewport_y_offset(), vw, vh);

        let image = self.splash_image.as_ref().unwrap();

        unsafe {
            let mut gl_ctx = self
                .internal_gl_ins
                .as_mut()
                .unwrap()
                .make_current_unchecked_for_window(
                    &mut |gl_ctx| self.window.gl_make_current(&gl_ctx.0).unwrap(),
                    &mut |s| self.video_ctx.gl_get_proc_address(s) as *const _,
                );

            use crate::gles::gles11_raw as gles11; // constants only

            let mut texture = 0;
            gl_ctx.GenTextures(1, &mut texture);
            gl_ctx.BindTexture(gles11::TEXTURE_2D, texture);
            let (width, height) = image.dimensions();
            gl_ctx.TexImage2D(
                gles11::TEXTURE_2D,
                0,
                gles11::RGBA as _,
                width as _,
                height as _,
                0,
                gles11::RGBA,
                gles11::UNSIGNED_BYTE,
                image.pixels().as_ptr() as *const _,
            );
            gl_ctx.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MIN_FILTER,
                gles11::LINEAR as _,
            );
            gl_ctx.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MAG_FILTER,
                gles11::LINEAR as _,
            );

            present_frame(
                gl_ctx.as_mut(),
                viewport,
                matrix,
                /* virtual_cursor_visible_at: */ None,
                /* focus_marker: */ None,
                /* debug_lines: */ &[],
                /* overlays: */ &[],
            );

            gl_ctx.DeleteTextures(1, &texture);
        };

        self.window.gl_swap_window();

        // hold onto GL context so the image doesn't disappear, and hold
        // onto image so we can rotate later if necessary
    }

    /// Swap front-buffer and back-buffer so the result of OpenGL rendering is
    /// presented.
    pub fn swap_window(&self) {
        self.window.gl_swap_window();
    }

    /// Consider the emulated device to be rotated to a particular orientation.
    ///
    /// On a PC or laptop, this will make the window be rotated so the app
    /// content appears upright. On a mobile device, this might do something
    /// else, because the user can physically rotate the screen.
    pub fn rotate_device(&mut self, new_orientation: DeviceOrientation) {
        assert!(self.on_main_stack);
        if new_orientation == self.device_orientation {
            return;
        }

        // A maximized window keeps its size (setting one would restore it);
        // the output is letterboxed to the new orientation instead.
        let maximized = self.window.window_flags()
            & sdl2_sys::SDL_WindowFlags::SDL_WINDOW_MAXIMIZED as u32
            != 0;
        if !self.fullscreen && !Self::rotatable_fullscreen() && !maximized {
            let (width, height) = if Self::rotatable_fullscreen() {
                set_sdl2_orientation(new_orientation);
                rotate_fullscreen_size(new_orientation, self.window.size())
            } else {
                size_for_orientation(self.device_family, new_orientation, self.scale_hack)
            };

            // macOS quirk: when resizing the window, the new framebuffer's size
            // is apparently max(new_size, old_size) in each dimension, but the
            // viewport is positioned wrong on the y axis for some reason, so we
            // need to apply an offset.
            // Recreating the OpenGL context was an alternative workaround, but
            // that apparently stops other OpenGL contexts drawing to the
            // framebuffer!
            #[cfg(target_os = "macos")]
            {
                let (_old_width, old_height) = self.window.size();
                self.max_height = self.max_height.max(old_height).max(height);
                self.viewport_y_offset = self.max_height - height;
            }

            self.window.set_size(width, height).unwrap();
        }

        if Self::rotatable_fullscreen() {
            set_sdl2_orientation(new_orientation);
            // Hack: from reading SDL2's source code, it seems that SDL2 will
            // only re-do the orientation when changing whether a window is
            // "resizeable" (can be rotated). You can't set the resizeable state
            // on a fullscreen window, so it must be temporarily stop being
            // fulscreen.
            // Apparently, doing this does result in resizing the window.
            self.window
                .set_fullscreen(sdl2::video::FullscreenType::Off)
                .unwrap();
            unsafe {
                let window_raw = self.window.raw();
                sdl2_sys::SDL_SetWindowResizable(window_raw, sdl2_sys::SDL_bool::SDL_FALSE);
                sdl2_sys::SDL_SetWindowResizable(window_raw, sdl2_sys::SDL_bool::SDL_TRUE);
            }
            self.window
                .set_fullscreen(sdl2::video::FullscreenType::True)
                .unwrap();
        }

        self.device_orientation = new_orientation;

        if self.splash_image.is_some() {
            self.display_splash();
        }
    }

    pub fn device_family(&self) -> DeviceFamily {
        self.device_family
    }

    /// Returns the current device orientation
    pub fn current_rotation(&self) -> DeviceOrientation {
        self.device_orientation
    }

    /// Get the size in pixels of the window without rotation or scaling.
    ///
    /// The aspect ratio, scale and orientation reflect the guest app's view of
    /// the world.
    pub fn size_unrotated_unscaled(&self) -> (u32, u32) {
        size_for_orientation(
            self.device_family,
            DeviceOrientation::Portrait,
            NonZeroU32::new(1).unwrap(),
        )
    }

    /// Get the region of the on-screen window (x, y, width, height) used to
    /// display the app content.
    ///
    /// The aspect ratio of this region always reflects the guest app's view of
    /// the world, but the scale and orientation might not.
    pub fn viewport(&self) -> (u32, u32, u32, u32) {
        let (app_width, app_height) =
            size_for_orientation(self.device_family, self.device_orientation, self.scale_hack);

        // Scale the iOS content viewport to fill the SDL drawable area while
        // preserving aspect ratio (letterbox/pillarbox the remainder). This
        // applies in both fullscreen AND windowed modes so resizing the
        // window scales the game smoothly. The rotation matrix (separate)
        // handles iOS-portrait → SDL-landscape orientation; viewport size
        // only affects where the content rectangle sits on screen.
        let (screen_width, screen_height) = self.window.drawable_size();
        if screen_width == 0 || screen_height == 0 {
            return (0, 0, app_width, app_height);
        }

        let app_aspect = app_width as f32 / app_height as f32;
        let screen_aspect = screen_width as f32 / screen_height as f32;
        let (scaled_width, scaled_height) = if app_aspect < screen_aspect {
            (
                (screen_height as f32 * app_aspect).round() as u32,
                screen_height,
            )
        } else {
            (
                screen_width,
                (screen_width as f32 / app_aspect).round() as u32,
            )
        };
        let x = screen_width.saturating_sub(scaled_width) / 2;
        let y = screen_height.saturating_sub(scaled_height) / 2;
        (x, y, scaled_width, scaled_height)
    }

    /// Special offset to add to y co-ordinates, only when drawing to screen.
    pub fn viewport_y_offset(&self) -> u32 {
        #[cfg(target_os = "macos")]
        return self.viewport_y_offset;
        #[cfg(not(target_os = "macos"))]
        return 0;
    }

    /// Transformation matrix for transforming between the window's co-ordinate
    /// space and the app's original co-ordinate space when rotation is in use
    /// (see [Self::rotate_device]). This returns a matrix appropriate for
    /// rotating texture co-ordinates to display the image in the window; when
    /// rotating input co-ordinates, invert the matrix.
    pub fn rotation_matrix(&self) -> Matrix<2> {
        match self.device_orientation {
            DeviceOrientation::Portrait => Matrix::identity(),
            DeviceOrientation::LandscapeLeft => Matrix::z_rotation(-FRAC_PI_2),
            DeviceOrientation::LandscapeRight => Matrix::z_rotation(FRAC_PI_2),
        }
    }

    pub fn is_screen_saver_enabled(&self) -> bool {
        self.video_ctx.is_screen_saver_enabled()
    }
    pub fn set_screen_saver_enabled(&mut self, enabled: bool) {
        assert!(self.on_main_stack);
        match enabled {
            true => self.video_ctx.enable_screen_saver(),
            false => self.video_ctx.disable_screen_saver(),
        }
    }

    pub fn start_text_input(&self) {
        assert!(self.on_main_stack);
        self.text_input_active.set(true);
        unsafe {
            sdl2_sys::SDL_StartTextInput();
        }
    }
    pub fn stop_text_input(&self) {
        assert!(self.on_main_stack);
        self.text_input_active.set(false);
        unsafe {
            sdl2_sys::SDL_StopTextInput();
        }
    }

    pub fn on_main_stack(&self) -> bool {
        self.on_main_stack
    }
}

pub fn open_url(env: &mut Environment, url: &str) -> Result<(), String> {
    env.on_parent_stack_in_coroutine(|_, _| sdl2::url::open_url(url).map_err(|e| e.to_string()))
}

/// Show an SDL messagebox for an error (typically after a panic).
///
/// The window argument allows for passing in the parent window for the
/// messagebox, which is not required but should be done if possible.
pub fn show_error_messagebox(window: Option<&Window>, error_message: &str) {
    assert!(window.is_none_or(|win| win.on_main_stack));
    use sdl2::messagebox;
    let mbox = [
        messagebox::ButtonData {
            flags: messagebox::MessageBoxButtonFlag::NOTHING,
            button_id: 0,
            text: "Open touchHLE directory",
        },
        messagebox::ButtonData {
            flags: messagebox::MessageBoxButtonFlag::NOTHING,
            button_id: 1,
            text: "Close",
        },
    ];

    let Ok(clicked_button) = messagebox::show_message_box(
        messagebox::MessageBoxFlag::ERROR,
        &mbox,
        "touchHLE crashed!",
        &format!("touchHLE crashed with the following error: {error_message}"),
        window.map(|win| &win.window),
        None,
    ) else {
        panic!("Failed to show message box!");
    };

    match clicked_button {
        messagebox::ClickedButton::CloseButton => {}
        messagebox::ClickedButton::CustomButton(button) => {
            match button.button_id {
                // Open data directory (contains log file on android)
                0 => match crate::paths::url_for_opening_user_data_dir() {
                    Ok(url) => {
                        if let Err(e) = sdl2::url::open_url(&url).map_err(|e| e.to_string()) {
                            echo!("Couldn't open file manager at {:?}: {}", url, e);
                        } else {
                            echo!("Opened file manager at {:?}, exiting.", url);
                        }
                    }
                    Err(e) => echo!("Couldn't open file manager: {}", e),
                },
                // Close
                1 => {}
                _ => unreachable!(),
            }
        }
    }
}

/// Get current battery state from SDL2.
///
/// Returns:
/// - pct: i32 - percentage of battery remaining.
/// - status: [BatteryState] - the current status of the battery
///   (unplugged, charging, full, etc.)
///
/// Must hop to the parent stack: on Android, SDL's implementation calls into
/// JNI (`Android_JNI_GetPowerInfo`), and ART's stack-overflow check looks at
/// the thread's recorded stack range — which the corosensei coroutine stack
/// isn't part of. Calling from the coroutine aborts with a JNI
/// `StackOverflowError`.
pub fn get_battery_status(env: &mut crate::Environment) -> (i32, BatteryState) {
    env.on_parent_stack_in_coroutine(|_, _| {
        let mut pct = 0;
        // Unfortunately, Rust-SDL2 does not expose this function yet.
        // iPhoneOS does not measure the battery in seconds remaining,
        // so we discard this argument.
        let status = unsafe { sdl2_sys::SDL_GetPowerInfo(null_mut(), &mut pct) };
        (
            pct,
            match status {
                SDL_PowerState::SDL_POWERSTATE_UNKNOWN => BatteryState::Unknown,
                SDL_PowerState::SDL_POWERSTATE_ON_BATTERY => BatteryState::OnBattery,
                SDL_PowerState::SDL_POWERSTATE_NO_BATTERY => BatteryState::NoBattery,
                SDL_PowerState::SDL_POWERSTATE_CHARGING => BatteryState::Charging,
                SDL_PowerState::SDL_POWERSTATE_CHARGED => BatteryState::Full,
            },
        )
    })
}

pub fn get_preferred_language_codes(env: &mut Environment) -> Vec<String> {
    env.on_parent_stack_in_coroutine(|_, _| {
        sdl2::locale::get_preferred_locales()
            .map(|loc| loc.lang)
            .collect()
    })
}

pub fn get_preferred_country_codes(env: &mut Environment) -> Vec<String> {
    env.on_parent_stack_in_coroutine(|_, _| {
        sdl2::locale::get_preferred_locales()
            .filter_map(|loc| loc.country)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (u32, u32) = (320, 480);

    #[test]
    fn screen_to_viewport_undoes_input_mapping() {
        // A landscape window, letterboxed, for each orientation.
        let viewport = (40, 10, 960, 640);
        for rotation in [
            Matrix::identity(),
            Matrix::z_rotation(-FRAC_PI_2),
            Matrix::z_rotation(FRAC_PI_2),
        ] {
            for point in [(0.0, 0.0), (320.0, 480.0), (100.0, 25.0), (160.0, 240.0)] {
                let window = screen_to_viewport(point, viewport, &rotation, SCREEN);
                let back = viewport_to_screen(window, viewport, &rotation, SCREEN);
                assert!(
                    (back.0 - point.0).abs() < 0.01 && (back.1 - point.1).abs() < 0.01,
                    "{point:?} -> {window:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn screen_centre_is_viewport_centre() {
        let viewport = (40, 10, 960, 640);
        let rotation = Matrix::z_rotation(-FRAC_PI_2);
        let (x, y) = screen_to_viewport((160.0, 240.0), viewport, &rotation, SCREEN);
        assert!((x - 520.0).abs() < 0.01 && (y - 330.0).abs() < 0.01, "{x} {y}");
    }

    #[test]
    fn stick_presses_past_half_and_releases_below_a_quarter() {
        assert_eq!(stick_dpad(None, 0.3, 0.0), None);
        assert_eq!(stick_dpad(None, 0.6, 0.0), Some(PadButton::DPadRight));
        assert_eq!(stick_dpad(None, -0.6, 0.0), Some(PadButton::DPadLeft));
        assert_eq!(stick_dpad(None, 0.0, -0.6), Some(PadButton::DPadUp));
        assert_eq!(stick_dpad(None, 0.0, 0.6), Some(PadButton::DPadDown));
        // Held part-way, it stays down until under 25%.
        let right = Some(PadButton::DPadRight);
        assert_eq!(stick_dpad(right, 0.4, 0.0), right);
        assert_eq!(stick_dpad(right, 0.26, 0.1), right);
        assert_eq!(stick_dpad(right, 0.2, 0.0), None);
        // From rest, part-way does nothing.
        assert_eq!(stick_dpad(None, 0.4, 0.4), None);
    }

    #[test]
    fn diagonals_take_the_axis_pushed_further() {
        assert_eq!(stick_dpad(None, 0.6, 0.7), Some(PadButton::DPadDown));
        assert_eq!(stick_dpad(None, -0.8, 0.7), Some(PadButton::DPadLeft));
        // Swinging round to the other axis switches direction.
        let right = Some(PadButton::DPadRight);
        assert_eq!(stick_dpad(right, 0.3, -0.9), Some(PadButton::DPadUp));
    }

    #[test]
    fn stick_and_dpad_together_press_once() {
        let mut d = Directions::default();
        assert_eq!(
            d.set_dpad(PadButton::DPadUp, true),
            Some((PadButton::DPadUp, true))
        );
        // The stick joins in: already down, nothing new.
        assert!(d.set_stick(Some(PadButton::DPadUp)).is_empty());
        // The D-pad lets go while the stick holds: still down.
        assert_eq!(d.set_dpad(PadButton::DPadUp, false), None);
        // The stick lets go: now it's released.
        assert_eq!(d.set_stick(None), vec![(PadButton::DPadUp, false)]);
    }

    #[test]
    fn stick_switching_direction_releases_then_presses() {
        let mut d = Directions::default();
        d.set_stick(Some(PadButton::DPadLeft));
        assert_eq!(
            d.set_stick(Some(PadButton::DPadUp)),
            vec![(PadButton::DPadLeft, false), (PadButton::DPadUp, true)]
        );
    }

    #[test]
    fn triggers_press_past_half_and_release_below_a_quarter() {
        // Pulled past half-way: pressed.
        assert_eq!(trigger_edge(false, 0.3), None);
        assert_eq!(trigger_edge(false, 0.51), Some(true));
        // Held part-way doesn't chatter: it stays down until under 25%.
        assert_eq!(trigger_edge(true, 0.4), None);
        assert_eq!(trigger_edge(true, 0.26), None);
        assert_eq!(trigger_edge(true, 0.2), Some(false));
        // Already up or down: nothing new.
        assert_eq!(trigger_edge(false, 0.0), None);
        assert_eq!(trigger_edge(true, 1.0), None);
    }
}
