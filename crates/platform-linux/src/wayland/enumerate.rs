//! Wayland display and window enumeration.
//!
//! Displays comes from the wl_output protocol, which reports what monitors are available.
//! Along with this, we request zxdg_output_manager_v1 to help get fractional scaling information.
//! After sending the initial requests to the compositor, we add events to the queue to
//! get the xdg output information of all monitors. Since scaling is reported as an integer,
//! we use the logical size and the display size to calculate the fractional scaling per monitor.


use pinray_core::{DisplaySource, Result, SourceId};

use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum, protocol::{wl_output::{self, WlOutput}, wl_registry::{self, WlRegistry}},
};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::{self, ZxdgOutputManagerV1},
    zxdg_output_v1::{self, ZxdgOutputV1}
};

use crate::wayland::platform_error;

#[derive(Default)]
struct WlState {
    outputs: Vec<DisplaySource>,
    wl_outputs: Vec<WlOutput>,
    zxdg_output_manager: Option<ZxdgOutputManagerV1>
}

impl Dispatch<WlRegistry, ()> for WlState {
    fn event(state: &mut Self, registry: &WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            if interface == "wl_output" {
                // user data = index into state.outputs, so events know where to go
                let idx = state.outputs.len();
                state.outputs.push(DisplaySource {
                    id: SourceId(String::from("undefined")),
                    name: String::from("undefined"),
                    width: 0,
                    height: 0,
                    scale_factor_milli: 0,
                    is_primary: false
                });
                let output = registry.bind::<WlOutput, _, _>(name, version.min(4), qh, idx);
                state.wl_outputs.push(output);
            }

            if interface == "zxdg_output_manager_v1" {
                state.zxdg_output_manager = Some(registry.bind::<ZxdgOutputManagerV1, _, _>(name, version.min(1), qh, ()));
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
            wl_output::Event::Name { name } => {
                let mut source = name.clone();
                source.insert_str(0, "display:");
                out.name = name; 
                out.id = SourceId(source);
            }, // v4+
            _ => {}
        }
    }
}

// This just sits here since it doesnt send events, we need it to bind the output manager
impl Dispatch<ZxdgOutputManagerV1, ()> for WlState {
    fn event(_state: &mut Self, _: &ZxdgOutputManagerV1, _event: zxdg_output_manager_v1::Event, _data: &(), _conn: &Connection, _qhandle: &QueueHandle<WlState>) {}
}

impl Dispatch<ZxdgOutputV1, usize> for WlState {
    fn event(state: &mut Self, _: &ZxdgOutputV1, event: zxdg_output_v1::Event, idx: &usize, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zxdg_output_v1::Event::LogicalSize { width, height: _ } => {
                let current_width_milli = state.outputs[*idx].width * 1000;
                let logical_width = width as u32;
                let scale_milli = current_width_milli / logical_width;
                state.outputs[*idx].scale_factor_milli = scale_milli;
            }
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

    if let Some(mgr) = &state.zxdg_output_manager {
        for (idx, output) in state.wl_outputs.iter().enumerate() {
            mgr.get_xdg_output(&output, &qh, idx);
        }
    }

    queue.roundtrip(&mut state).map_err(platform_error)?; // output geometry/mode/name events

    Ok(state.outputs)
}