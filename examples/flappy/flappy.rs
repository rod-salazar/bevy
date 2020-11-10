use bevy::{
    diagnostic::{Diagnostics, FrameTimeDiagnosticsPlugin},
    prelude::*,
};

// A unit struct to help identify the FPS UI component, since there may be many Text components
struct FpsText;

fn main() {
    App::build()
        .add_plugins(DefaultPlugins)
        .add_plugin(FrameTimeDiagnosticsPlugin::default())
        .add_startup_system(setup_fps_text.system())
        .add_startup_system(setup_game.system())
        .add_system(text_update_system.system())
        .add_system(sprite_render.system())
        .run();
}

fn setup_game(
    commands: &mut Commands,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let texture_handle = asset_server.load("flappy/tanks/tanks_tankGreen1.png");
    let texture_handle2 = asset_server.load("flappy/tanks/tanks_tankGreen2.png");
    commands
        .spawn(Camera2dComponents::default())
        .spawn(SpriteComponents {
            material: materials.add(texture_handle.into()),
            ..Default::default()
        })
        .with(SpriteComponents {
            material: materials.add(texture_handle2.into()),
            ..Default::default()
        });
}

fn sprite_render(query: Query<&Sprite>) {
    let mut x = 0;
    for sprite in query.iter() {
        x += 1;
    }
    println!("{}", x);
}

fn setup_fps_text(commands: &mut Commands, asset_server: Res<AssetServer>) {
    commands
        // UI camera
        .spawn(UiCameraComponents::default())
        // texture
        .spawn(TextComponents {
            style: Style {
                align_self: AlignSelf::FlexEnd,
                ..Default::default()
            },
            text: Text {
                value: "FPS:".to_string(),
                font: asset_server.load("fonts/FiraSans-Bold.ttf"),
                style: TextStyle {
                    font_size: 60.0,
                    color: Color::WHITE,
                },
            },
            ..Default::default()
        })
        .with(FpsText)
        .with(Timer::from_seconds(0.5, true));
}

fn text_update_system(
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
