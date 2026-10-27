use pinray_core::{DisplaySource, PinrayError, Result, SourceId, WindowSource};

use wayland_client::{
    protocol::{wl_output::{self, WlOutput}, wl_registry::{self, WlRegistry}},
    Connection, Dispatch, QueueHandle, WEnum,
};

use crate::wayland::platform_error;

#[derive(Default)]
struct WlState {
    outputs: Vec<DisplaySource>,
}

impl Dispatch<WlRegistry, ()> for WlState {
    fn event(state: &mut Self, registry: &WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            if interface == "wl_output" {
                // user data = index into state.outputs, so events know where to go
                let idx = state.outputs.len();
                state.outputs.push( DisplaySource { 
                    id: SourceId(String::from("null")),
                    name: String::default(),
                    width: 0,
                    height: 0,
                    scale_factor_milli: 1000,
                    is_primary: false,
                });
                registry.bind::<WlOutput, _, _>(name, version.min(4), qh, idx);
            }
        }
    }
}

impl Dispatch<WlOutput, usize> for WlState {
    fn event(state: &mut Self, _: &WlOutput, event: wl_output::Event, idx: &usize, _: &Connection, _: &QueueHandle<Self>) {
        let out = &mut state.outputs[*idx];
        match event {
            wl_output::Event::Mode { flags: WEnum::Value(f), width, height, .. } if f.contains(wl_output::Mode::Current) => {
                out.width = width as u32;
                out.height = height as u32;
            }
            wl_output::Event::Scale { factor } => out.scale_factor_milli = factor as u32,
            wl_output::Event::Name { name } => out.name = name, // v4+
            // wl_output::Event::Description { description } => out.description = description,
            _ => {}
        }
    }
}
pub(super) fn enumerate_monitors() -> Result<Vec<DisplaySource>> { 

    let conn = Connection::connect_to_env().map_err(platform_error)?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = WlState::default();
    queue.roundtrip(&mut state).map_err(platform_error)?; // registry globals → bind outputs
    queue.roundtrip(&mut state).map_err(platform_error)?; // output geometry/mode/name events

    Ok(state.outputs)
}