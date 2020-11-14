use crate::TileKind::Dirt;
use bevy::{
    diagnostic::{Diagnostics, FrameTimeDiagnosticsPlugin},
    prelude::*,
    utils::HashMap,
};

// Right now this is in pixels, but it should just be "units"
// and we should have a separate variable on pixels per unit that
// changes when you zoom in and out.
const SQ_SIZE: u32 = 30;

// Col and Row represent the position of the square on the visible screen, not
// on the overall plane.
struct DrawSq {
    col: u32,
    row: u32,
}

// Represents a rectangle with a bottom left at x,y
struct Rect {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

struct ViewRect(Rect);

#[derive(Clone)]
enum TileKind {
    Dirt,
}

#[derive(Clone)]
struct TileSq {
    tile: TileKind,
}

// This should represent # of SQ_SIZE that can fit
const SECTOR_ROWS: u32 = 20;
const SECTOR_COLS: u32 = 20;

// Represents a giant square in the world
struct WorldSector {
    tiles: Vec<TileSq>, // entire row * cols
    // Indices (can be negative) of the current sector.
    x: i32,
    y: i32,
}

impl WorldSector {
    fn new() -> Self {
        WorldSector {
            // How to verify this stores it in 1 giant block of memory?
            tiles: vec![TileSq { tile: Dirt }; (SECTOR_COLS * SECTOR_ROWS) as usize],
            x: 0,
            y: 0,
        }
    }
}

// Several different blocks of contiguous memory used to hold the world tile types
struct World {
    sectors: HashMap<i32, HashMap<i32, WorldSector>>,
}

impl World {
    fn sectors_from_world_rect() {}
}

// This will be more complex once we have a 'zoom level'
fn screen_rect_to_world(center: &ViewRect) -> &Rect {
    &center.0
}

fn main() {
    App::build()
        .add_resource(WindowDescriptor {
            vsync: false,
            ..Default::default()
        })
        .add_plugins(DefaultPlugins)
        .add_plugin(FrameTimeDiagnosticsPlugin::default())
        .add_startup_system(setup_fps_text.system())
        .add_system(fps_text_update_system.system())
        .add_startup_system(setup_grid.system())
        .add_system(update_grid.system())
        .run();
}

fn update_grid() {}

fn setup_grid(
    commands: &mut Commands,
    windows: Res<Windows>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let window = windows.get_primary().unwrap();
    let width = window.width();
    let height = window.height();
    // +1 for the width not being divisble by grid, another +1 for 1 sq of buffer when panning
    let grid_rows = (height / SQ_SIZE) + 2;
    let grid_cols = (width / SQ_SIZE) + 2;
    let width: f32 = width as f32;
    let height: f32 = height as f32;

    let brown_material = materials
        .add(Color::rgb(101.0f32 / 255.0f32, 67.0f32 / 255.0f32, 63.0f32 / 255.0f32).into());
    commands
        .spawn(Camera2dComponents::default())
        // Red dot for helpful alignment
        .spawn(SpriteComponents {
            material: materials.add(Color::rgb(1.0f32, 0.0f32 / 0.0f32, 0.0f32 / 255.0f32).into()),
            transform: Transform::from_translation(Vec3::new(0.0, 0.0, 0.0)),
            sprite: Sprite::new(Vec2::new(2 as f32, 2 as f32)),
            ..Default::default()
        })
        .spawn((ViewRect(Rect {
            x: 0,
            y: 0,
            w: width as u32,
            h: height as u32,
        }),));

    // This creates the Grid objects that will get recycled by swapping out the material
    // as we pan around. Should experiment with using a shader.
    let mut x = 0;
    for i in 0..grid_rows {
        for j in 0..grid_cols {
            x += 1;
            let sq = DrawSq { row: i, col: j };
            commands
                .spawn(SpriteComponents {
                    material: brown_material.clone(),
                    transform: Transform::from_translation(calc_screen_pos(width, height, &sq)),
                    sprite: Sprite::new(Vec2::new(SQ_SIZE as f32, SQ_SIZE as f32)),
                    ..Default::default()
                })
                .with(sq);
        }
    }
}

fn calc_screen_pos(width: f32, height: f32, sq: &DrawSq) -> Vec3 {
    Vec3::new(
        -((width / 2.0f32) / SQ_SIZE as f32) * SQ_SIZE as f32
            + (SQ_SIZE as f32 / 2.0f32)
            + (SQ_SIZE as f32 * sq.col as f32),
        -((height / 2.0f32) / SQ_SIZE as f32) * SQ_SIZE as f32
            + (SQ_SIZE as f32 / 2.0f32)
            + (SQ_SIZE as f32 * sq.row as f32),
        0.0f32,
    )
}

// fn size_scaling(windows: Res<Windows>, mut q: Query<(&Size, &mut Sprite)>) {
//     let window = windows.get_primary().unwrap();
//     for (sprite_size, mut sprite) in q.iter_mut() {
//         sprite.size = Vec2::new(
//             sprite_size.width / ARENA_WIDTH as f32 * window.width() as f32,
//             sprite_size.height / ARENA_HEIGHT as f32 * window.height() as f32,
//         );
//     }
// }

// A unit struct to help identify the FPS UI component, since there may be many Text components
struct FpsText;

fn setup_fps_text(commands: &mut Commands, asset_server: Res<AssetServer>) {
    commands
        // UI camera
        .spawn(UiCameraComponents::default())
        // texture
        .spawn(TextComponents {
            node: Default::default(),
            style: Style {
                align_self: AlignSelf::FlexEnd,
                ..Default::default()
            },
            draw: Default::default(),
            text: Text {
                value: "FPS:".to_string(),
                font: asset_server.load("fonts/FiraSans-Bold.ttf"),
                style: TextStyle {
                    font_size: 60.0,
                    color: Color::WHITE,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        })
        .with(FpsText)
        .with(Timer::from_seconds(0.5, true));
}

fn fps_text_update_system(
    diagnostics: Res<Diagnostics>,
    mut query: Query<(&mut Text, &FpsText, &Timer)>,
) {
    for (mut text, _tag, timer) in query.iter_mut() {
        if !timer.finished {
            continue;
        }
        if let Some(fps) = diagnostics.get(FrameTimeDiagnosticsPlugin::FPS) {
            if let Some(average) = fps.average() {
                text.value = format!("FPS: {:.2}", average);
            }
        }
    }
}
