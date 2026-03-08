use indexmap::IndexMap;
use rayon::prelude::*;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::fs::canonicalize;
use std::io::BufReader;

use go_patterns_common::baduk::{GoBoard, Player, pack_games};

mod load_sgfs;
use load_sgfs::load_all_sgfs;

mod find_duplicates;
use find_duplicates::find_duplicates;

fn main() {
    let mut sgf_folder = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    sgf_folder.push("../../frontend/public/sgfs");
    sgf_folder = canonicalize(sgf_folder).unwrap();
    println!("Loading games from '{sgf_folder:?}' ...");

    let games_vec = load_all_sgfs(&sgf_folder);

    // Collect all player names from all games (including duplicates)
    println!("Collecting all player names...");
    let mut all_names = std::collections::HashSet::new();
    for (_, game) in &games_vec {
        match &game.player_black {
            Player::Id(_, name) => all_names.insert(name.clone()),
            Player::Unknown(name) if !name.is_empty() => all_names.insert(name.clone()),
            _ => false,
        };
        match &game.player_white {
            Player::Id(_, name) => all_names.insert(name.clone()),
            Player::Unknown(name) if !name.is_empty() => all_names.insert(name.clone()),
            _ => false,
        };
    }

    // Write all names to file
    println!("Writing all_names.txt...");
    let mut all_names: Vec<_> = all_names.into_iter().collect();
    all_names.sort();
    std::fs::write(
        "python-player-name-aliases/all_names.txt",
        all_names.join("\n"),
    )
    .unwrap();

    // Find duplicates and get the unique games and possible aliases
    let (final_unique_games, possible_aliases) = find_duplicates(games_vec);

    println!("Computing captures...");

    let mut games: IndexMap<String, _> = final_unique_games
        .into_par_iter()
        .map(|(path, mut game)| {
            let mut captures = HashMap::new();
            let mut gb = GoBoard::new();

            for (i, move_) in game.moves.iter().enumerate() {
                let cs = gb.make_move(move_);
                if !cs.is_empty() {
                    captures.insert(i, cs);
                }
            }
            game.captures = captures;
            (path, game)
        })
        .collect();

    games.sort_by(|_ka, game_a, _kb, game_b| {
        match (&game_a.date, &game_b.date) {
            (Some(date_a), Some(date_b)) => date_a.cmp(date_b),
            (Some(_), None) => std::cmp::Ordering::Less, // Games with dates come first
            (None, Some(_)) => std::cmp::Ordering::Greater, // Games without dates come last
            (None, None) => std::cmp::Ordering::Equal,   // Equal if both have no date
        }
    });

    println!("Writing games.pack...");
    let buf = pack_games(&games);
    std::fs::write("../wasm-search/src/games.pack", buf).unwrap();

    let metadata = std::fs::metadata("../wasm-search/src/games.pack").unwrap();
    let file_size = metadata.len();
    println!("games.pack size: {:.2} MB", file_size as f64 / 1_048_576.0);

    println!("Collecting unknown names from unique games...");
    let mut unknown_names = std::collections::HashSet::new();
    for game in games.values() {
        if let Player::Unknown(name) = &game.player_black {
            if !name.is_empty() {
                unknown_names.insert(name.clone());
            }
        }
        if let Player::Unknown(name) = &game.player_white {
            if !name.is_empty() {
                unknown_names.insert(name.clone());
            }
        }
    }

    println!("Writing unknown_names.txt...");
    let mut unknown_names: Vec<_> = unknown_names.into_iter().collect();
    unknown_names.sort();
    std::fs::write("unknown_names.txt", unknown_names.join("\n")).unwrap();

    // Count player ID usage across unique games
    let mut player_id_counts = HashMap::<i16, usize>::new();
    for game in games.values() {
        if let Player::Id(id, _) = game.player_black {
            *player_id_counts.entry(id).or_insert(0) += 1;
        }
        if let Player::Id(id, _) = game.player_white {
            *player_id_counts.entry(id).or_insert(0) += 1;
        }
    }

    println!("Writing possible_aliases.txt...");
    let mut aliases: Vec<_> = possible_aliases
        .into_iter()
        .filter(|alias| {
            let count1 = player_id_counts.get(&alias.id1).unwrap_or(&0);
            let count2 = player_id_counts.get(&alias.id2).unwrap_or(&0);
            *count1 > 0 && *count2 > 0
        })
        .collect();
    aliases.sort_by(|a, b| a.id1.cmp(&b.id1).then(a.id2.cmp(&b.id2)));
    let aliases_str = aliases
        .iter()
        .map(|a| format!("{} {}", a.id1, a.id2))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write("possible_aliases.txt", aliases_str).unwrap();

    println!("Updating player_names.json with games count...");
    update_player_names_with_games_count(&player_id_counts);

    println!("Done");
}

fn update_player_names_with_games_count(player_id_counts: &HashMap<i16, usize>) {
    let file = File::open("python-player-name-aliases/player_names.json")
        .expect("Failed to open player names file");
    let reader = BufReader::new(file);
    let mut json: Value =
        serde_json::from_reader(reader).expect("Failed to parse player names JSON");

    if let Some(players_obj) = json.as_object_mut() {
        for (player_id_str, player_data) in players_obj.iter_mut() {
            if let Ok(player_id) = player_id_str.parse::<i16>() {
                if let Some(player_obj) = player_data.as_object_mut() {
                    let games_count = player_id_counts.get(&player_id).unwrap_or(&0);
                    player_obj.insert(
                        "games_count".to_string(),
                        Value::Number(serde_json::Number::from(*games_count)),
                    );
                }
            }
        }
    }

    // Write the updated JSON back to the file
    let output_file = File::create("python-player-name-aliases/player_names.json")
        .expect("Failed to create player names file");
    serde_json::to_writer_pretty(output_file, &json)
        .expect("Failed to write updated player names JSON");
}
