use bevy::{
    diagnostic::{Diagnostics, FrameTimeDiagnosticsPlugin},
    prelude::*,
    render::texture::{TextureFormat, TextureFormat::Rgba8UnormSrgb},
    sprite::TextureAtlasBuilder,
    utils::{AHashExt, HashSet},
};
use rand::Rng;
use std::time::Duration;

/**
The plan is to design a Chunk system. The Chunk system is for storing world tiles in a way that they
can be worked with efficiently with the ecs system and as a way to incrementally load the world as
you pan/zoom around.

Each Chunk:
- Has a square grid of Tile structs
- Each Tile has a texture handle (and perhaps game specific metadata)
- Each Chunk has a texture with a size equal to all the tiles combined.
- We can change the texture handle on the underlying Tiles independently.
- Each Chunk will be an entity and we will have a system which prepares the current texture,
  by iterating over all the tiles and copying over into the large Chunk texture.
- Chunks will not move in position in the world.
- When the camera moves we will determine which chunks should be on-screen and which should not,
  using this information we will create new Chunk entities and de-spawn sufficently off-screen
  chunks.
- Data for the overall world will not be stored in chunks but instead in a lighter weight compact
  data-structure.
 */

trait Tile {
    fn texture(&self) -> &Handle<ColorMaterial>;
}

const CHUNK_WIDTH: u32 = 16; // How many tiles in each chunk ROW
const TILE_WIDTH: u32 = 16; // units, not necessarily pixels, but equal to pixels at default zoom

trait Chunk<T: Tile> {
    fn tiles(&self) -> &Vec<T>;
    fn x(&self) -> i32;
    fn y(&self) -> i32;
}

struct Vec2 {
    x: f32,
    y: f32,
}

impl Vec2 {
    fn new(x: f32, y: f32) -> Self {
        Vec2 { x, y }
    }
}

struct Rect {
    /// The beginning point of the rect
    pub min: Vec2,
    /// The ending point of the rect
    pub max: Vec2,
}

// Gives us chunks are in a given world rect
fn world_rect_to_chunk_indices(rect: Rect) -> HashSet<(i32, i32)> {
    let corner1 = world_point_to_chunk_index(rect.max.x, rect.max.y);
    let corner2 = world_point_to_chunk_index(rect.min.x, rect.min.y);

    let minx = std::cmp::min(corner1.0, corner2.0);
    let maxx = std::cmp::max(corner1.0, corner2.0);

    let miny = std::cmp::min(corner1.1, corner2.1);
    let maxy = std::cmp::max(corner1.1, corner2.1);

    let mut chunks = HashSet::new();
    for i in minx..(maxx + 1) {
        for j in miny..(maxy + 1) {
            chunks.insert((i, j));
        }
    }
    chunks
}

fn world_point_to_chunk_index(x: f32, y: f32) -> (i32, i32) {
    // Does this behave right for negatives?
    // No zoom handling necessary since this is a world point
    let x = x / (CHUNK_WIDTH as f32 * TILE_WIDTH as f32);
    let y = y / (CHUNK_WIDTH as f32 * TILE_WIDTH as f32);

    (x.floor() as i32, y.floor() as i32)
}

fn screen_info_to_world_rect(width: f32, height: f32, center_x: f32, center_y: f32) -> Rect {
    // Will support zoom in the future
    Rect {
        max: Vec2::new(center_x - width / 2.0f32, center_y + height / 2.0f32),
        min: Vec2::new(center_x + width / 2.0f32, center_y - height / 2.0f32),
    }
}

fn chunk_index_to_world_pos_center(chunk_x_index: i32, chunk_y_index: i32) -> (f32, f32) {
    let chunk_width_units = (CHUNK_WIDTH * TILE_WIDTH) as f32;
    let offset_to_center = chunk_width_units / 2.0f32;
    (
        chunk_width_units * chunk_x_index as f32 + offset_to_center,
        chunk_width_units * chunk_y_index as f32 + offset_to_center,
    )
}

/*
Example of chunk position
                 -
                 -
                 -+++++++++
                 -+++++++++
                 -++(0,0)++
                 -+++++++++
------------------------------------
                 -
                 -
                 -
 */

// ===================================================================

#[derive(Clone)]
enum FlappyTileKind {
    Dirt,
}

struct FlappyTile {
    texture: Handle<ColorMaterial>,
    kind: FlappyTileKind,
}

impl Clone for FlappyTile {
    fn clone(&self) -> Self {
        FlappyTile {
            texture: self.texture.clone(),
            kind: self.kind.clone(),
        }
    }

    fn clone_from(&mut self, _source: &Self) {
        unimplemented!()
    }
}

impl Tile for FlappyTile {
    fn texture(&self) -> &Handle<ColorMaterial> {
        &self.texture
    }
}

struct FlappyChunk<T: Tile> {
    tiles: Vec<T>,

    // Indices correspond to chunk indices, not world index
    x: i32,
    y: i32,
}

impl<T: Tile> Chunk<T> for FlappyChunk<T> {
    fn tiles(&self) -> &Vec<T> {
        &self.tiles
    }

    fn x(&self) -> i32 {
        self.x
    }

    fn y(&self) -> i32 {
        self.y
    }
}

struct MainCamera;
struct Center(f32, f32);
struct InputTimer(Timer);

fn main() {
    App::build()
        .add_resource(WindowDescriptor {
            vsync: false,
            ..Default::default()
        })
        .add_resource(Center(0.0f32, 0.0f32))
        .add_resource(InputTimer(Timer::new(
            Duration::from_millis(25. as u64),
            true,
        )))
        .add_plugins(DefaultPlugins)
        .add_plugin(FrameTimeDiagnosticsPlugin::default())
        .add_startup_system(setup_game)
        .add_startup_system(setup_fps_text)
        .add_startup_system(setup_texture_atlas)
        .add_stage("chunk_management")
        .add_system_to_stage("chunk_management", chunk_management)
        .add_stage_after("chunk_management", "drawing_chunk")
        .add_system_to_stage("drawing_chunk", update_chunk_textures)
        .add_system(fps_text_update_system)
        .add_system(handle_input.system())
        .run();
}

fn handle_input(
    mut input_timer: ResMut<InputTimer>,
    time: ResMut<Time>,
    keyboard_input: Res<Input<KeyCode>>,
    mut center: ResMut<Center>,
    mut q: Query<(Mut<Transform>,), With<MainCamera>>,
) {
    input_timer.0.tick(time.delta_seconds);

    // Look into bevy_contrib_schedules as a replacement
    if !input_timer.0.finished {
        return;
    }

    let dx = if keyboard_input.pressed(KeyCode::Left) {
        -6
    } else if keyboard_input.pressed(KeyCode::Right) {
        6
    } else {
        0
    };

    let dy = if keyboard_input.pressed(KeyCode::Down) {
        -6
    } else if keyboard_input.pressed(KeyCode::Up) {
        6
    } else {
        0
    };

    if dx == 0 && dy == 0 {
        return;
    }

    center.0 += dx as f32;
    center.1 += dy as f32;

    for (mut t,) in q.iter_mut() {
        t.translation.set_x(center.0);
        t.translation.set_y(center.1);
    }
}

fn setup_game(
    commands: &mut Commands,
    mut materials: ResMut<Assets<ColorMaterial>>,
    windows: Res<Windows>,
) {
    let window = windows.get_primary().unwrap();
    let width = window.width();
    let height = window.height();
    println!("Window: {}/{}", width, height);

    commands
        .spawn(Camera2dBundle::default())
        .with(MainCamera {})
        // Red dot for helpful alignment
        .spawn(SpriteBundle {
            material: materials.add(Color::rgb(1.0f32, 0.0f32 / 0.0f32, 0.0f32 / 255.0f32).into()),
            transform: Transform::from_translation(Vec3::new(0.0, 0.0, 1.0)),
            sprite: Sprite::new(bevy::prelude::Vec2::new(2 as f32, 2 as f32)),
            ..Default::default()
        })
        //Another at the right side of the first Chunk
        .spawn(SpriteBundle {
            material: materials.add(Color::rgb(1.0f32, 0.0f32 / 0.0f32, 0.0f32 / 255.0f32).into()),
            transform: Transform::from_translation(Vec3::new(
                (CHUNK_WIDTH * TILE_WIDTH) as f32,
                0.0,
                1.0,
            )),
            sprite: Sprite::new(bevy::prelude::Vec2::new(2 as f32, 2 as f32)),
            ..Default::default()
        });
}

fn setup_texture_atlas(
    mut mut_textures: ResMut<Assets<Texture>>,
    mut mut_texture_atlases: ResMut<Assets<TextureAtlas>>,
) {
    let width = TILE_WIDTH as f32;
    let num_textures = 2.0f32;
    let mut atlas_builder = TextureAtlasBuilder::new(
        bevy::prelude::Vec2::new(width, width),
        bevy::prelude::Vec2::new(width * num_textures, width),
    );

    let brown = create_brown_texture(TILE_WIDTH, TILE_WIDTH);
    let green = create_green_texture(TILE_WIDTH, TILE_WIDTH);

    let brown_handle = mut_textures.add(brown);
    let green_handle = mut_textures.add(green);
    let brown = mut_textures.get(brown_handle.clone()).unwrap();
    let green = mut_textures.get(green_handle.clone()).unwrap();
    atlas_builder.add_texture(brown_handle, brown);
    atlas_builder.add_texture(green_handle, green);

    let atlas = atlas_builder.finish(&mut *mut_textures).unwrap();
    mut_texture_atlases.add(atlas);
}

fn update_chunk_textures(
    mut mut_textures: ResMut<Assets<Texture>>,
    materials: ResMut<Assets<ColorMaterial>>,
    q: Query<(&Handle<ColorMaterial>, &FlappyChunk<FlappyTile>)>,
) {
    for (material, chunk) in q.iter() {
        // No clone
        let chunk_material = materials.get(material).unwrap();
        let chunk_pixel_format_size = {
            let chunk_texture = mut_textures
                .get_mut(chunk_material.texture.as_ref().unwrap())
                .unwrap();
            chunk_texture.format.pixel_size() as u32
        };

        let bytes_per_tile_row = TILE_WIDTH * chunk_pixel_format_size;
        let bytes_per_chunk_row = CHUNK_WIDTH * bytes_per_tile_row;

        for (tile_i, tile) in chunk.tiles.iter().enumerate() {
            // For each Tile
            let tile_i = tile_i as u32;
            let tile_row = tile_i as u32 / CHUNK_WIDTH;
            let chunk_tex_tile_top_left = (tile_row * bytes_per_chunk_row * CHUNK_WIDTH)
                + ((tile_i % CHUNK_WIDTH) * bytes_per_tile_row);

            let tile_material = materials.get(tile.texture.clone()).unwrap();
            let tile_texture_handle = match tile_material.texture {
                None => {
                    panic!("No texture found inside of tile_material")
                }
                Some(_) => tile_material.texture.as_ref().unwrap().clone(),
            };

            let tile_texture = {
                // Gross, copy since we can't have 2 open textures which came from the bevy
                // assets resource. Maybe open issue. Play with unsafe.
                mut_textures.get(tile_texture_handle).unwrap().clone()
            };

            // // What's this .clone for, or as_ref?
            let chunk_texture = mut_textures
                .get_mut(chunk_material.texture.as_ref().unwrap())
                .unwrap();

            for tile_inner_row_i in 0..TILE_WIDTH {
                // For each row in the tile
                let chunk_position_row_begin =
                    (chunk_tex_tile_top_left + (bytes_per_chunk_row * tile_inner_row_i)) as usize;
                let chunk_position_row_end =
                    (chunk_position_row_begin + bytes_per_tile_row as usize) as usize; // end exclusive.

                let tile_pos_start = (bytes_per_tile_row * tile_inner_row_i) as usize;
                let tile_pos_end = tile_pos_start + bytes_per_tile_row as usize;

                debug_assert_eq!(
                    chunk_position_row_end - chunk_position_row_begin,
                    tile_pos_end - tile_pos_start
                );
                debug_assert_eq!(
                    (chunk_position_row_end - chunk_position_row_begin)
                        % chunk_pixel_format_size as usize,
                    0
                );

                // todo: assert on color format

                // does copy from slice work with the same speed or faster than clone_from_slice?
                chunk_texture.data[chunk_position_row_begin..chunk_position_row_end]
                    .copy_from_slice(&tile_texture.data[tile_pos_start..tile_pos_end]);
            }
        }
    }
}

fn create_brown_texture(pixel_width: u32, pixel_height: u32) -> Texture {
    let color = vec![210u8, 105u8, 30u8, 255u8];
    create_color_texture(&color, pixel_width, pixel_height)
}

fn create_green_texture(pixel_width: u32, pixel_height: u32) -> Texture {
    let color = vec![0u8, 255u8, 0u8, 255u8];
    create_color_texture(&color, pixel_width, pixel_height)
}

// Create brown sRGB square texture
fn create_color_texture(color_bytes: &[u8], pixel_width: u32, pixel_height: u32) -> Texture {
    Texture::new_fill(
        bevy::prelude::Vec2::new(pixel_width as f32, pixel_height as f32),
        &color_bytes,
        Rgba8UnormSrgb,
    )
}

fn chunk_management(
    commands: &mut Commands,
    windows: Res<Windows>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut textures: ResMut<Assets<Texture>>,
    center: Res<Center>,
    q: Query<(Entity, &FlappyChunk<FlappyTile>)>,
    mut counter_q: Query<(Mut<ChunkCounter>,)>,
) {
    let window = windows.get_primary().unwrap();
    let width = window.width();
    let height = window.height();

    let world_rect = screen_info_to_world_rect(width as f32, height as f32, center.0, center.1);
    let next_chunk_indices = world_rect_to_chunk_indices(world_rect);
    for (mut cc,) in counter_q.iter_mut() {
        cc.0 = next_chunk_indices.len() as u32;
    }
    let mut current_chunk_indices = HashSet::new();
    for (entity, flappy_chunk) in q.iter() {
        if !next_chunk_indices.contains(&(flappy_chunk.x(), flappy_chunk.y())) {
            // println!("de-spawning {} {}", flappy_chunk.x(), flappy_chunk.y());
            commands.despawn(entity);
        } else {
            // It's current minus the ones that will be de-spawned anyway
            current_chunk_indices.insert((flappy_chunk.x(), flappy_chunk.y()));
        }
    }

    let mut rng = rand::thread_rng();

    for next_index in next_chunk_indices {
        if !current_chunk_indices.contains(&next_index) {
            // Should this be placed somewhere cached, like a resource?
            let brown_texture =
                ColorMaterial::texture(textures.add(create_brown_texture(TILE_WIDTH, TILE_WIDTH)));
            let brown_material = materials.add(brown_texture);
            let green_texture =
                ColorMaterial::texture(textures.add(create_green_texture(TILE_WIDTH, TILE_WIDTH)));
            let green_material = materials.add(green_texture);

            let mut tiles = vec![];
            for _i in 0..CHUNK_WIDTH * CHUNK_WIDTH {
                let r: u8 = rng.gen();
                tiles.push(FlappyTile {
                    texture: if r % 2 == 1 {
                        brown_material.clone()
                    } else {
                        green_material.clone()
                    }, // This is the per tile texture
                    kind: FlappyTileKind::Dirt,
                });
            }
            let chunk_texture_size = bevy::prelude::Vec2::new(
                (CHUNK_WIDTH * TILE_WIDTH) as f32,
                (CHUNK_WIDTH * TILE_WIDTH) as f32,
            );
            let texture = textures.add(Texture::new(
                chunk_texture_size.clone(),
                vec![0u8; ((CHUNK_WIDTH * TILE_WIDTH) * (CHUNK_WIDTH * TILE_WIDTH) * 4) as usize],
                TextureFormat::Rgba8UnormSrgb,
            ));
            let chunk_texture = materials.add(ColorMaterial::texture(texture));

            let translate = chunk_index_to_world_pos_center(next_index.0, next_index.1);
            // println!(
            //     "spawning {} {} @ {} {}",
            //     next_index.0, next_index.1, translate.0, translate.1
            // );
            commands
                .spawn(SpriteBundle {
                    material: chunk_texture, // This should be the big chunk texture
                    transform: Transform::from_translation(Vec3::new(
                        translate.0,
                        translate.1,
                        0.0f32,
                    )),
                    sprite: Sprite::new(chunk_texture_size),
                    ..Default::default()
                })
                .with(FlappyChunk {
                    tiles,
                    x: next_index.0,
                    y: next_index.1,
                });
        }
    }
}

// A unit struct to help identify the FPS UI component, since there may be many Text components
struct FpsText;
struct ChunkCounter(u32);

fn setup_fps_text(commands: &mut Commands, asset_server: Res<AssetServer>) {
    commands
        // UI camera
        .spawn(UiCameraBundle::default())
        // texture
        .spawn(TextBundle {
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
                    color: Color::BLUE,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        })
        .with(ChunkCounter(0))
        .with(FpsText)
        .with(Timer::from_seconds(0.5, true));
}

fn fps_text_update_system(
    diagnostics: Res<Diagnostics>,
    mut query: Query<(Mut<Text>, &FpsText, &Timer, &ChunkCounter)>,
) {
    for (mut text, _tag, timer, chunk_counter) in query.iter_mut() {
        if !timer.finished {
            continue;
        }
        if let Some(fps) = diagnostics.get(FrameTimeDiagnosticsPlugin::FPS) {
            if let Some(average) = fps.average() {
                text.value = format!("FPS/CHUNKS: {:.2}/{}", average, chunk_counter.0);
            }
        }
    }
}
