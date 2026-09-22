//! The few places the Spine runtime needs `unsafe`: its C-side user pointers and vertex math.
#![allow(unsafe_code)]

use rusty_spine::c::c_void;
use std::sync::Once;

use rusty_spine::Skeleton;

/// What an atlas page's renderer object holds: the page's file name.
struct PageName(String);

/// Tags every atlas page loaded from now on with its file name, so a renderable can say which
/// page it draws from. The runtime's hook is global; it is set once.
pub(crate) fn name_pages() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        rusty_spine::extension::set_create_texture_cb(|page, _path| {
            let name = page.name().to_owned();
            page.renderer_object().set(PageName(name));
        });
        rusty_spine::extension::set_dispose_texture_cb(|page| unsafe {
            page.renderer_object().dispose::<PageName>();
        });
    });
}

/// The page name a renderable's renderer object points at.
pub(crate) fn page_name(object: Option<*const c_void>) -> Option<String> {
    let ptr = object?;
    // SAFETY: page renderer objects are only ever set by the callback `name_pages` installs (no
    // other code may install one: the runtime's hook is global, last writer wins), always to a
    // `PageName`; atlases loaded before it ran have none and never get here. It lives as long as
    // its atlas, which every skeleton drawing from it keeps alive (skeleton data holds the atlas).
    Some(unsafe { &*ptr.cast::<PageName>() }.0.clone())
}

/// World vertices (Spine space, y up) of each bounding box whose slot or attachment name
/// contains `name`.
pub(crate) fn bounding_boxes(skeleton: &Skeleton, name: &str) -> Vec<Vec<[f32; 2]>> {
    let mut polygons = Vec::new();
    for slot in skeleton.slots() {
        let Some(attachment) = slot.attachment() else {
            continue;
        };
        let Some(bounds) = attachment.as_bounding_box() else {
            continue;
        };
        if !slot.data().name().contains(name) && !attachment.name().contains(name) {
            continue;
        }
        let count = bounds.world_vertices_length();
        let mut world = vec![0.0; count as usize];
        // SAFETY: the attachment is this slot's own, and `world` holds all its vertices.
        unsafe {
            bounds.compute_world_vertices(&slot, 0, count, &mut world, 0, 2);
        }
        polygons.push(world.chunks_exact(2).map(|c| [c[0], c[1]]).collect());
    }
    polygons
}
