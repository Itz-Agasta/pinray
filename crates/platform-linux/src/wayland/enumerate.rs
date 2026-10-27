//! Wayland display enumeration.
//!
//! Displays come from the wl_output globals from the compositor, which
//! reports what monitors are available. wl_output only reports integer
//! scaling, so we also bind zxdg_output_manager_v1 to read each monitor's
//! logical size and compute the scale manually against the physical size.
//!
//! Wayland has no primary output; the first advertised one is reported as
//! primary so callers that look for `is_primary` keep working.

use pinray_core::{DisplaySource, Result, SourceId};

use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{
        wl_output::{self, Transform, WlOutput},
        wl_registry::{self, WlRegistry},
    },
};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::{self, ZxdgOutputManagerV1},
    zxdg_output_v1::{self, ZxdgOutputV1},
};

use crate::wayland::platform_error;

// for things that dont fit in display source but still need to be tracked
struct DisplayInfo {
    transform: WEnum<Transform>,
    has_name: bool,
}

#[derive(Default)]
struct WlState {
    outputs: Vec<DisplaySource>,
    wl_outputs: Vec<WlOutput>,
    display_info: Vec<DisplayInfo>,
    zxdg_output_manager: Option<ZxdgOutputManagerV1>,
}

impl Dispatch<WlRegistry, ()> for WlState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "wl_output" {
                // user data = index into state.outputs, so events know where to go
                let idx = state.outputs.len();
                state.outputs.push(DisplaySource {
                    id: SourceId(format!("display:undefined (index {})", idx)),
                    name: format!("undefined (index {})", idx),
                    width: 0,
                    height: 0,
                    scale_factor_milli: 1000,
                    is_primary: idx == 0,
                });
                state.display_info.push(DisplayInfo {
                    transform: WEnum::Value(Transform::Normal),
                    has_name: false,
                });

                let output = registry.bind::<WlOutput, _, _>(name, version.min(4), qh, idx);
                state.wl_outputs.push(output);
            }

            if interface == "zxdg_output_manager_v1" {
                state.zxdg_output_manager =
                    Some(registry.bind::<ZxdgOutputManagerV1, _, _>(name, version.min(3), qh, ()));
            }
        }
    }
}

impl Dispatch<WlOutput, usize> for WlState {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let out = &mut state.outputs[*idx];
        match event {
            wl_output::Event::Mode {
                flags: WEnum::Value(f),
                width,
                height,
                ..
            } if f.contains(wl_output::Mode::Current) => {
                out.width = width as u32;
                out.height = height as u32;
            }
            wl_output::Event::Name { name } => {
                let mut source = name.clone();
                source.insert_str(0, "display:");
                out.name = name;
                out.id = SourceId(source);
                state.display_info[*idx].has_name = true;
            } // v4+
            wl_output::Event::Geometry {
                x: _,
                y: _,
                physical_width: _,
                physical_height: _,
                subpixel: _,
                make: _,
                model: _,
                transform,
            } => state.display_info[*idx].transform = transform,
            _ => {}
        }
    }
}

// This just sits here since it doesnt send events, we need it to bind the output manager
impl Dispatch<ZxdgOutputManagerV1, ()> for WlState {
    fn event(
        _state: &mut Self,
        _: &ZxdgOutputManagerV1,
        _event: zxdg_output_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<WlState>,
    ) {
    }
}

impl Dispatch<ZxdgOutputV1, usize> for WlState {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                let current_width_milli = state.outputs[*idx].width * 1000;

                // If the monitor is rotated vertically, the height is the real width
                let physical_width = match state.display_info[*idx].transform.into_result() {
                    Ok(Transform::_90)
                    | Ok(Transform::_270)
                    | Ok(Transform::Flipped90)
                    | Ok(Transform::Flipped270) => height,
                    _ => width,
                };

                if physical_width == 0 || current_width_milli == 0 {
                    return;
                }

                let logical_width = physical_width as u32;
                let scale_milli = current_width_milli / logical_width;
                state.outputs[*idx].scale_factor_milli = scale_milli;
            }
            zxdg_output_v1::Event::Name { name } => {
                if !state.display_info[*idx].has_name {
                    let mut source = name.clone();
                    source.insert_str(0, "display:");
                    state.outputs[*idx].name = name;
                    state.outputs[*idx].id = SourceId(source);
                    state.display_info[*idx].has_name = true;
                }
            }
            _ => {}
        }
    }
}

/// Lists the compositor's outputs using a short-lived Wayland connection.
///
/// Three roundtrips: registry globals (bind outputs and the xdg-output
/// manager), then output mode/name events, then xdg-output logical sizes.
/// The xdg outputs are requested only after the second roundtrip so each
/// output's physical width is known before its logical size arrives.
pub(super) fn enumerate_monitors() -> Result<Vec<DisplaySource>> {
    let conn = Connection::connect_to_env().map_err(platform_error)?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = WlState::default();
    queue.roundtrip(&mut state).map_err(platform_error)?; // registry globals -> bind outputs
    queue.roundtrip(&mut state).map_err(platform_error)?; // output geometry/mode/name events

    if let Some(mgr) = &state.zxdg_output_manager {
        for (idx, output) in state.wl_outputs.iter().enumerate() {
            mgr.get_xdg_output(output, &qh, idx);
        }
    }

    queue.roundtrip(&mut state).map_err(platform_error)?; // output zxdg logical size events

    Ok(state.outputs)
}
