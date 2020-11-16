use bevy::{prelude::*, render::texture::TextureFormat};

fn main() {
    App::build()
        .add_plugins(DefaultPlugins)
        .add_startup_system(setup.system())
        .add_system(my_system.system())
        .add_system(my_system2.system())
        .run();
}

fn setup(
    commands: &mut Commands,
    mut textures: ResMut<Assets<Texture>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    // Large enough to not take too long to OOM
    let square_side_len = 1000;

    // Create a square texture resource, and a material resource around that.
    let chunk_texture_size = Vec2::new(square_side_len as f32, square_side_len as f32);
    let texture = textures.add(Texture::new(
        chunk_texture_size.clone(),
        vec![0u8; (square_side_len * square_side_len * 4) as usize], // * 4 due to color format px size
        TextureFormat::Rgba8UnormSrgb,
    ));
    let chunk_texture = materials.add(ColorMaterial::texture(texture));

    commands.spawn(SpriteComponents {
        material: chunk_texture, // This should be the big chunk texture
        sprite: Sprite::new(chunk_texture_size),
        ..Default::default()
    });
}

fn my_system(
    mut textures: ResMut<Assets<Texture>>,
    materials: ResMut<Assets<ColorMaterial>>,
    q: Query<(&Handle<ColorMaterial>,)>,
) {
    for (m,) in q.iter() {
        let chunk_material = materials.get(m).unwrap();

        // This seems to leak somehow
        for _i in 0..5 {
            let handle_ref = chunk_material.texture.as_ref().unwrap();
            let _tex = textures.get_mut(handle_ref);

            // Using get() instead of get_mut() would not OOM
            // let _tex = textures.get(handle_ref);
        }
        println!("Done with system this frame!");
    }
}

fn my_system2(
    mut textures: ResMut<Assets<Texture>>,
    materials: ResMut<Assets<ColorMaterial>>,
    q: Query<(&Handle<ColorMaterial>,)>,
) {
    for (m,) in q.iter() {
        let chunk_material = materials.get(m).unwrap();

        // This seems to leak somehow
        for _i in 0..10000 {
            let handle_ref = chunk_material.texture.as_ref().unwrap();
            let _tex = textures.get_mut(handle_ref);

            // Using get() instead of get_mut() would not OOM
            // let _tex = textures.get(handle_ref);
        }
        println!("2 Done with system this frame!");
    }
}
