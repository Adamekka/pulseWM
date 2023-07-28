// pulseWM is not a snake case name
#![allow(non_snake_case)]

mod data;
mod state;

use std::{
    ffi::OsString,
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::Arc,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        self,
        input::{InputEvent, KeyState, KeyboardKeyEvent},
        renderer::{
            damage::OutputDamageTracker, element::surface::WaylandSurfaceRenderElement,
            gles::GlesRenderer,
        },
        winit::{self, WinitEvent},
    },
    desktop::{space::render_output, Space, Window},
    input::{
        keyboard::{keysyms, FilterResult, KeysymHandle},
        Seat, SeatState,
    },
    output,
    reexports::{
        calloop::{
            generic::Generic,
            timer::{TimeoutAction, Timer},
            EventLoop, Interest, Mode, PostAction,
        },
        wayland_server::{Display, DisplayHandle},
    },
    utils::{Physical, Serial, Size, Transform, SERIAL_COUNTER},
    wayland::{
        compositor::CompositorState, data_device::DataDeviceState, output::OutputManagerState,
        shell::xdg::XdgShellState, shm::ShmState, socket::ListeningSocketSource,
    },
};

fn main() {
    let mut event_loop: EventLoop<data::Data> =
        EventLoop::try_new().expect("Failed to create EventLoop");

    let mut display: Display<state::State> = Display::new().unwrap();

    let socket: ListeningSocketSource = ListeningSocketSource::new_auto().unwrap();
    let socket_name: OsString = socket.socket_name().to_os_string();

    event_loop
        .handle()
        .insert_source(socket, |stream: UnixStream, _, data: &mut data::Data| {
            data.display
                .handle()
                .insert_client(stream, Arc::new(data::ClientData::default()))
                .unwrap();
        })
        .unwrap();

    event_loop
        .handle()
        .insert_source(
            Generic::new(
                display.backend().poll_fd().as_raw_fd(),
                Interest::READ,
                Mode::Level,
            ),
            |_, _, data: &mut data::Data| {
                data.display.dispatch_clients(&mut data.state).unwrap();
                Ok(PostAction::Continue)
            },
        )
        .unwrap();

    let display_handle: DisplayHandle = display.handle();

    let compositor_state: CompositorState = CompositorState::new::<state::State>(&display_handle);
    let shm_state = ShmState::new::<state::State>(&display_handle, Vec::new());
    let output_manager_state: OutputManagerState =
        OutputManagerState::new_with_xdg_output::<state::State>(&display_handle);
    let xdg_shell_state: XdgShellState = XdgShellState::new::<state::State>(&display_handle);
    let mut seat_state: SeatState<state::State> = SeatState::<state::State>::new();
    let space: Space<Window> = Space::<Window>::default();
    let data_device_state: DataDeviceState = DataDeviceState::new::<state::State>(&display_handle);

    let mut seat: Seat<state::State> = seat_state.new_wl_seat(&display_handle, "pulseWM_seat");
    seat.add_keyboard(Default::default(), 500, 500).unwrap();
    seat.add_pointer();

    let state: state::State = state::State {
        compositor_state,
        data_device_state,
        seat_state,
        shm_state,
        space,
        output_manager_state,
        xdg_shell_state,
    };

    let mut data: data::Data = data::Data { state, display };

    let (mut backend, mut winit) = winit::init::<GlesRenderer>().unwrap();

    let size: Size<i32, Physical> = backend.window_size().physical_size;

    let mode: output::Mode = output::Mode {
        size,
        refresh: 60_000,
    };

    // Doesn't matter, winit takes care of it
    let psychical_properties: output::PhysicalProperties = output::PhysicalProperties {
        size: (0, 0).into(),
        subpixel: output::Subpixel::Unknown,
        make: "pulseWM".into(),
        model: "pulseWM-Winit".into(),
    };

    let output: output::Output =
        output::Output::new("pulseWM-winit".to_string(), psychical_properties);
    output.create_global::<state::State>(&data.display.handle());
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    data.state.space.map_output(&output, (0, 0));

    std::env::set_var("WAYLAND_DISPLAY", socket_name);

    let start_time: Instant = std::time::Instant::now();
    let timer: Timer = Timer::immediate();

    let mut output_damage_tracker = OutputDamageTracker::from_output(&output);

    event_loop
        .handle()
        .insert_source(timer, move |_, _, data: &mut data::Data| {
            let display = &mut data.display;
            let state = &mut data.state;

            winit
                .dispatch_new_events(|event: winit::WinitEvent| {
                    if let WinitEvent::Input(event) = event {
                        if let InputEvent::Keyboard { event } = event {
                            let serial: Serial = SERIAL_COUNTER.next_serial();
                            let time: u32 = backend::input::Event::time_msec(&event);
                            let press_state = event.state();
                            let action = seat.get_keyboard().unwrap().input::<u8, _>(
                                state,
                                event.key_code(),
                                press_state,
                                serial,
                                time,
                                |_, _, keysym: KeysymHandle<'_>| {
                                    if press_state == KeyState::Pressed
                                        && keysym.modified_sym() == keysyms::KEY_t | keysyms::KEY_T
                                    {
                                        FilterResult::Intercept(1)
                                    } else {
                                        FilterResult::Forward
                                    }
                                },
                            );

                            if Some(1) == action {
                                std::process::Command::new("alacritty")
                                    .spawn()
                                    .expect("Failed to spawn alacritty");
                            }
                        }
                    }
                })
                .unwrap();

            backend.bind().unwrap();

            render_output::<_, WaylandSurfaceRenderElement<GlesRenderer>, _, _>(
                &output,
                backend.renderer(),
                1_f32,
                0,
                [&state.space],
                &[],
                &mut output_damage_tracker,
                [0.1, 0.1, 0.1, 1.0],
            )
            .unwrap();

            backend.submit(None).unwrap();

            state.space.elements().for_each(|window: &Window| {
                window.send_frame(
                    &output,
                    start_time.elapsed(),
                    Some(Duration::ZERO),
                    |_, _| Some(output.clone()),
                )
            });

            state.space.refresh();

            display.flush_clients().unwrap();

            TimeoutAction::ToDuration(Duration::from_millis(16))
        })
        .unwrap();

    event_loop.run(None, &mut data, |_| {}).unwrap();
}
