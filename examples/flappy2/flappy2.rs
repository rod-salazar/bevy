use bevy::{
    diagnostic::{Diagnostics, FrameTimeDiagnosticsPlugin},
    prelude::*,
    render::texture::{Extent3d, TextureDimension, TextureFormat, TextureFormat::Rgba8UnormSrgb},
    sprite::TextureAtlasBuilder,
    tasks::{TaskPool, TaskPoolBuilder},
    utils::{AHashExt, HashMap, HashSet},
};
///use futures_lite::pin;
use rand::Rng;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

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
    fn texture(&self) -> &Handle<Texture>;
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
    Grass,
}

struct FlappyTile {
    texture: Handle<Texture>,
    rect: bevy::sprite::Rect,
    kind: FlappyTileKind,
}

impl Clone for FlappyTile {
    fn clone(&self) -> Self {
        FlappyTile {
            texture: self.texture.clone(),
            rect: self.rect.clone(),
            kind: self.kind.clone(),
        }
    }

    fn clone_from(&mut self, _source: &Self) {
        unimplemented!()
    }
}

impl Tile for FlappyTile {
    fn texture(&self) -> &Handle<Texture> {
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

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
enum TextureName {
    DIRT,
    GRASS,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
enum TextureAtlasName {
    LANDSCAPE,
}

struct TextureAtlasLookup(HashMap<TextureAtlasName, Handle<TextureAtlas>>);
struct TextureAtlasTexLookup(HashMap<TextureName, Handle<Texture>>);

struct ChunkPool(TaskPool);

trait Creator<T: Sync> {
    fn create(&self) -> T;
}

struct ChunkTextureCreator {}

impl Creator<Texture> for ChunkTextureCreator {
    fn create(&self) -> Texture {
        println!("Allocating chunk texture");
        create_black_texture(CHUNK_WIDTH * TILE_WIDTH, CHUNK_WIDTH * TILE_WIDTH)
    }
}

struct ArenaBar<T: Sync, C: Creator<T>> {
    pool: Vec<T>,
    creator: C,
}

impl<T: Sync, C: Creator<T>> ArenaBar<T, C> {
    fn new(size: u32, creator: C) -> Self {
        let mut pool = vec![];
        for _ in 0..size {
            let t = creator.create();
            pool.push(t);
        }

        ArenaBar { pool, creator }
    }

    // Always creates one otherwise.
    fn pop(&mut self) -> T {
        match self.pool.pop() {
            None => self.creator.create(),
            Some(value) => value,
        }
    }

    fn push(&mut self, value: T) {
        self.pool.push(value);
    }
}

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
        .add_resource(TextureAtlasLookup(HashMap::new()))
        .add_resource(TextureAtlasTexLookup(HashMap::new()))
        .add_resource(ChunkPool(
            TaskPoolBuilder::new()
                .thread_name("Chunk Pool".to_string())
                .build(),
        ))
        .add_resource(ArenaBar::new(80, ChunkTextureCreator {}))
        .add_plugins(DefaultPlugins)
        .add_plugin(FrameTimeDiagnosticsPlugin::default())
        // Setup
        .add_startup_system(setup_game)
        .add_startup_system(setup_fps_text)
        .add_startup_system(setup_texture_atlas)
        // Regular stages
        .add_stage("chunk_management")
        .add_system_to_stage("chunk_management", chunk_management)
        .add_stage_after("chunk_management", "drawing_chunk")
        .add_system_to_stage("drawing_chunk", update_chunk_textures)
        .add_system(fps_text_update_system)
        .add_system(handle_input)
        .run();
}

fn handle_input(
    mut input_timer: ResMut<InputTimer>,
    time: ResMut<Time>,
    keyboard_input: Res<Input<KeyCode>>,
    mut center: ResMut<Center>,
    mut q: Query<(Mut<Transform>,), With<MainCamera>>,
) {
    input_timer.0.tick(time.delta_seconds());

    // Look into bevy_contrib_schedules as a replacement
    if !input_timer.0.finished() {
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
        t.translation.x = center.0;
        t.translation.y = center.1;
    }
}

fn setup_game(
    commands: &mut Commands,
    // mut materials: ResMut<Assets<ColorMaterial>>,
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
    // .spawn(SpriteBundle {
    //     material: materials.add(Color::rgb(1.0f32, 0.0f32 / 0.0f32, 0.0f32 / 255.0f32).into()),
    //     transform: Transform::from_translation(Vec3::new(0.0, 0.0, 1.0)),
    //     sprite: Sprite::new(bevy::prelude::Vec2::new(2 as f32, 2 as f32)),
    //     ..Default::default()
    // })
    //Another at the right side of the first Chunk
    // .spawn(SpriteBundle {
    //     material: materials.add(Color::rgb(1.0f32, 0.0f32 / 0.0f32, 0.0f32 / 255.0f32).into()),
    //     transform: Transform::from_translation(Vec3::new(
    //         (CHUNK_WIDTH * TILE_WIDTH) as f32,
    //         0.0,
    //         1.0,
    //     )),
    //     sprite: Sprite::new(bevy::prelude::Vec2::new(2 as f32, 2 as f32)),
    //     ..Default::default()
    // })
    ;
}

fn setup_texture_atlas(
    mut mut_textures: ResMut<Assets<Texture>>,
    mut mut_texture_atlases: ResMut<Assets<TextureAtlas>>,
    mut mut_texture_atlas_lookup: ResMut<TextureAtlasLookup>,
    mut mut_texture_atlas_tex_lookup: ResMut<TextureAtlasTexLookup>,
) {
    let width = TILE_WIDTH as f32;
    let num_textures = 2.0f32;
    let mut atlas_builder = TextureAtlasBuilder::new(
        bevy::prelude::Vec2::new(width, width),
        bevy::prelude::Vec2::new(width * num_textures, width),
    );

    let brown = create_brown_texture(TILE_WIDTH, TILE_WIDTH);
    let green = create_green_texture(TILE_WIDTH, TILE_WIDTH);

    // Seems like I have to actually register them as assets to use the AtlasBuilder.
    let brown_handle = mut_textures.add(brown);
    let green_handle = mut_textures.add(green);
    let brown = mut_textures.get(brown_handle.clone()).unwrap();
    let green = mut_textures.get(green_handle.clone()).unwrap();
    atlas_builder.add_texture(brown_handle.clone(), brown);
    atlas_builder.add_texture(green_handle.clone(), green);

    let atlas = atlas_builder.finish(&mut *mut_textures).unwrap();
    let atlas_handle = mut_texture_atlases.add(atlas);

    mut_textures.remove(brown_handle.clone());
    mut_textures.remove(green_handle.clone());

    mut_texture_atlas_lookup
        .0
        .insert(TextureAtlasName::LANDSCAPE, atlas_handle.clone());

    mut_texture_atlas_tex_lookup
        .0
        .insert(TextureName::DIRT, brown_handle);
    mut_texture_atlas_tex_lookup
        .0
        .insert(TextureName::GRASS, green_handle);
}

fn fetch_texture_by_name(
    atlas_name: &TextureAtlasName,
    name: &TextureName,
    texture_atlas_lookup: &TextureAtlasLookup,
    texture_atlas_tex_lookup: &TextureAtlasTexLookup,
    texture_atlases: &Assets<TextureAtlas>,
) -> (Handle<Texture>, bevy::sprite::Rect) {
    let atlas_handle = texture_atlas_lookup.0.get(&atlas_name).unwrap();

    let atlas = texture_atlases.get(atlas_handle).unwrap();
    let dirt_handle = texture_atlas_tex_lookup.0.get(&name).unwrap();
    let dirt_index = atlas.get_texture_index(dirt_handle).unwrap();

    let dirt = atlas.textures[dirt_index];
    (atlas.texture.clone(), dirt)
}

fn update_chunk_textures(
    mut textures: ResMut<Assets<Texture>>,
    materials: ResMut<Assets<ColorMaterial>>,
    pool: Res<ChunkPool>,
    mut arena: ResMut<ArenaBar<Texture, ChunkTextureCreator>>,
    q: Query<(&Handle<ColorMaterial>, &FlappyChunk<FlappyTile>)>,
) {
    let mut tasks = vec![];
    let new_textures = Arc::new(Mutex::new(HashMap::new()));
    for (chunk_material, chunk) in q.iter() {
        let chunk_texture_handle = {
            let chunk_material = materials.get(chunk_material.clone()).unwrap();
            chunk_material.texture.as_ref().unwrap().clone()
        };
        let srgb_pixel_format_size = {
            let chunk_texture = textures.get(chunk_texture_handle.clone()).unwrap();
            chunk_texture.format.pixel_size() as u32
        };

        let bytes_per_tile_row = TILE_WIDTH * srgb_pixel_format_size;
        let bytes_per_chunk_row = CHUNK_WIDTH * bytes_per_tile_row;

        let mut tile_texture_map = HashMap::new();
        let mut copied = false;

        for tile in chunk.tiles.iter() {
            tile_texture_map.entry(tile.texture.id).or_insert_with(|| {
                if copied {
                    panic!("Did not expect more than 1 copy");
                }
                copied = true;
                textures.get(tile.texture.clone()).unwrap().clone()
            });
        }

        let mut chunk_texture = arena.pop();
        let new_textures = new_textures.clone();
        let clone_and_update = async move {
            // SAD allocate and clone. If we want to use multi-threading then we need to clone since
            // taking a mutable borrow on the texture means the future does as well,
            // but then only 1 future at a time can take a mutable borrow since Assets
            // API at the moment makes you take the borrow on the entire thing.

            for (tile_i, tile) in chunk.tiles.iter().enumerate() {
                // For each Tile
                let tile_i = tile_i as u32;
                let tile_row = tile_i as u32 / CHUNK_WIDTH;
                let chunk_tex_tile_top_left = (tile_row * bytes_per_chunk_row * CHUNK_WIDTH)
                    + ((tile_i % CHUNK_WIDTH) * bytes_per_tile_row);

                // Copy once per frame
                let tile_texture = tile_texture_map.get(&tile.texture.id).unwrap();

                let tile_rect = &tile.rect;
                let bytes_per_atlas_row =
                    tile_texture.size.width as usize * srgb_pixel_format_size as usize;

                for tile_inner_row_i in 0..TILE_WIDTH {
                    // For each row in the tile
                    let chunk_position_row_begin = (chunk_tex_tile_top_left
                        + (bytes_per_chunk_row * tile_inner_row_i))
                        as usize;
                    let chunk_position_row_end =
                        (chunk_position_row_begin + bytes_per_tile_row as usize) as usize; // end exclusive.

                    // print to verify
                    let tile_atlas_start_pos = bytes_per_atlas_row * (tile_rect.min.y as usize)
                        + tile_rect.min.x as usize * srgb_pixel_format_size as usize;
                    let tile_pos_start =
                        tile_atlas_start_pos + (bytes_per_atlas_row * tile_inner_row_i as usize);
                    let tile_pos_end = tile_pos_start + bytes_per_tile_row as usize;

                    debug_assert_eq!(
                        chunk_position_row_end - chunk_position_row_begin,
                        tile_pos_end - tile_pos_start
                    );
                    debug_assert_eq!(
                        (chunk_position_row_end - chunk_position_row_begin)
                            % srgb_pixel_format_size as usize,
                        0
                    );
                    // todo: assert on color format

                    // does copy from slice work with the same speed or faster than clone_from_slice?
                    chunk_texture.data[chunk_position_row_begin..chunk_position_row_end]
                        .copy_from_slice(&tile_texture.data[tile_pos_start..tile_pos_end]);
                }
            }
            let mut new_textures = new_textures.lock().unwrap();
            new_textures.insert(chunk_texture_handle.clone(), chunk_texture);
        };
        tasks.push(clone_and_update);
    }

    pool.0.scope(|s| {
        for task in tasks {
            s.spawn(async move {
                task.await;
            });
        }
    });

    let mut new_textures = new_textures.lock().unwrap();
    for (handle, texture) in new_textures.drain() {
        let old = textures.swap(handle.clone(), texture).unwrap();
        arena.push(old);
    }
}

fn create_black_texture(pixel_width: u32, pixel_height: u32) -> Texture {
    let color = vec![0u8, 0u8, 0u8, 255u8];
    create_color_texture(&color, pixel_width, pixel_height)
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
        Extent3d {
            width: pixel_width,
            height: pixel_height,
            depth: 1,
        },
        TextureDimension::D2,
        &color_bytes,
        Rgba8UnormSrgb,
    )
}

fn chunk_management(
    commands: &mut Commands,
    windows: Res<Windows>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut textures: ResMut<Assets<Texture>>,
    texture_atlases: Res<Assets<TextureAtlas>>,
    center: Res<Center>,
    q: Query<(Entity, &FlappyChunk<FlappyTile>)>,
    mut counter_q: Query<(Mut<ChunkCounter>,)>,
    texture_atlas_lookup: Res<TextureAtlasLookup>,
    texture_atlas_tex_lookup: Res<TextureAtlasTexLookup>,
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
            //println!("de-spawning {} {}", flappy_chunk.x(), flappy_chunk.y());
            commands.despawn(entity);
        } else {
            // It's current minus the ones that will be de-spawned anyway
            current_chunk_indices.insert((flappy_chunk.x(), flappy_chunk.y()));
        }
    }

    let mut rng = rand::thread_rng();

    let (brown_texture_handle, brown_rect) = fetch_texture_by_name(
        &TextureAtlasName::LANDSCAPE,
        &TextureName::DIRT,
        &texture_atlas_lookup,
        &texture_atlas_tex_lookup,
        &texture_atlases,
    );
    let (green_texture_handle, green_rect) = fetch_texture_by_name(
        &TextureAtlasName::LANDSCAPE,
        &TextureName::GRASS,
        &texture_atlas_lookup,
        &texture_atlas_tex_lookup,
        &texture_atlases,
    );

    for next_index in next_chunk_indices {
        if !current_chunk_indices.contains(&next_index) {
            let mut tiles = vec![];
            for _i in 0..CHUNK_WIDTH * CHUNK_WIDTH {
                let r: u8 = rng.gen();
                tiles.push(FlappyTile {
                    texture: if r % 2 == 1 {
                        brown_texture_handle.clone()
                    } else {
                        green_texture_handle.clone()
                    }, // This is the per tile texture
                    rect: if r % 2 == 1 { brown_rect } else { green_rect },
                    kind: if r % 2 == 1 {
                        FlappyTileKind::Dirt
                    } else {
                        FlappyTileKind::Grass
                    },
                });
            }
            let chunk_texture_size = bevy::prelude::Vec2::new(
                (CHUNK_WIDTH * TILE_WIDTH) as f32,
                (CHUNK_WIDTH * TILE_WIDTH) as f32,
            );
            let texture = textures.add(Texture::new(
                Extent3d {
                    width: CHUNK_WIDTH * TILE_WIDTH,
                    height: CHUNK_WIDTH * TILE_WIDTH,
                    depth: 1,
                },
                TextureDimension::D2,
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
struct FpsAverage(f32, u32); // running average, seconds total

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
        .with(Timer::from_seconds(1.0f32, true))
        .with(FpsAverage(0f32, 0));
}

fn fps_text_update_system(
    diagnostics: Res<Diagnostics>,
    mut query: Query<(Mut<Text>, &FpsText, &Timer, &ChunkCounter, Mut<FpsAverage>)>,
) {
    for (mut text, _tag, timer, chunk_counter, mut fps_average) in query.iter_mut() {
        if !timer.finished() {
            continue;
        }
        // assumes timer is @ 1s interval

        if let Some(fps) = diagnostics.get(FrameTimeDiagnosticsPlugin::FPS) {
            if let Some(average) = fps.average() {
                fps_average.0 = ((fps_average.0 * fps_average.1 as f32) + average as f32)
                    / (fps_average.1 + 1) as f32;
                fps_average.1 += 1;
                text.value = format!(
                    "FPS/CHUNKS/AVG/SECS: {:.2}/{}/{}/{}",
                    average, chunk_counter.0, fps_average.0, fps_average.1
                );
            }
        }
    }
}
