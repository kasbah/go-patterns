use rayon::prelude::*;
use serde_json::Value;
use sgf_parse::{ParseOptions, go, parse_with_options};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use walkdir::WalkDir;

use go_patterns_common::baduk::{
    BOARD_SIZE, Color, Game, GameResult, Placement, Player, Point, Rank, parse_komi, parse_rank,
    parse_rules, parse_sgf_date, parse_sgf_result,
};

pub fn load_all_sgfs(sgf_folder: &PathBuf) -> Vec<(String, Game)> {
    let player_aliases = load_player_aliases();
    let blocklist = load_blocklist();
    let mut paths = Vec::new();

    println!("Loading games...");
    collect_sgf_files(sgf_folder, &mut paths, &blocklist);
    println!("Read directories");

    for path in &paths {
        println!("Loading {path:?} ...");
    }

    let mut games_vec = paths
        .par_iter()
        .filter_map(|path| match std::fs::read_to_string(path) {
            Ok(file_data) => match load_sgf(path, &file_data) {
                Ok((mut game, player_black, player_white)) => {
                    // Replace player names with id
                    game.player_black = find_player_id(&player_black, &player_aliases);
                    game.player_white = find_player_id(&player_white, &player_aliases);
                    let rel_path = path
                        .strip_prefix(sgf_folder)
                        .unwrap()
                        .with_extension("")
                        .to_string_lossy()
                        .into_owned();
                    Some((rel_path, game))
                }
                Err(e) => {
                    println!("Skipping {path:?}: {e}");
                    None
                }
            },
            Err(e) => {
                println!("Skipping {path:?}: {e}");
                None
            }
        })
        .collect::<Vec<_>>();

    games_vec.sort_by(|a, b| a.0.cmp(&b.0));
    games_vec
}

fn collect_sgf_files(base_dir: &PathBuf, paths: &mut Vec<PathBuf>, blocklist: &HashSet<String>) {
    for entry in WalkDir::new(base_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "sgf"))
    {
        let path = entry.path();
        let rel_path = path
            .strip_prefix(base_dir)
            .unwrap()
            .with_extension("")
            .to_string_lossy()
            .into_owned();

        if !blocklist.contains(&rel_path) {
            paths.push(path.to_path_buf());
        } else {
            println!("Skipping blocked path: {rel_path}");
        }
    }
}

fn load_player_aliases() -> HashMap<String, i16> {
    let mut aliases = HashMap::new();
    let file = File::open("python-player-name-aliases/player_names.json")
        .expect("Failed to open player names file");
    let reader = BufReader::new(file);
    let json: Value = serde_json::from_reader(reader).expect("Failed to parse player names JSON");

    let players_obj = json.as_object().expect("Expected object");
    for (player_id_str, player_data) in players_obj.iter() {
        let id = player_id_str
            .parse::<i16>()
            .expect("Failed to parse player ID");

        if let Some(aliases_array) = player_data.get("aliases").and_then(|a| a.as_array()) {
            for alias in aliases_array {
                if let Some(name) = alias.get("name").and_then(|n| n.as_str()) {
                    aliases.insert(name.to_lowercase(), id);
                }
            }
        }
    }
    aliases
}

fn find_player_id(name: &str, aliases: &HashMap<String, i16>) -> Player {
    let name = name
        .replace(['\n'], " ")
        .replace([','], "")
        .trim()
        .to_string();

    if name.is_empty() {
        return Player::Unknown("".to_string());
    }

    if let Some(id) = aliases.get(name.to_lowercase().as_str()) {
        return Player::Id(*id, name);
    }

    Player::Unknown(name)
}

fn has_multiple_players(name: &str) -> bool {
    name.contains(" and ")
        || name.contains("&")
        || name.matches(',').count() > 1
        || name.contains("day 1")
}

fn load_blocklist() -> HashSet<String> {
    match std::fs::read_to_string("blocklist.txt") {
        Ok(contents) => contents.lines().map(String::from).collect(),
        Err(_) => HashSet::new(),
    }
}

fn load_sgf(
    path: &PathBuf,
    file_data: &str,
) -> Result<(Game, String, String), Box<dyn std::error::Error>> {
    let parse_options = ParseOptions {
        lenient: true,
        ..ParseOptions::default()
    };
    let gametrees = parse_with_options(file_data, &parse_options)?;
    let game = gametrees
        .into_iter()
        .map(|gametree| gametree.into_go_node())
        .collect::<Result<Vec<_>, _>>()?;

    let mut moves = Vec::new();
    let mut event = String::new();
    let mut round = String::new();
    let mut location = String::new();
    let mut date = None;
    let mut player_black = String::new();
    let mut player_white = String::new();
    let mut rank_black = Rank::Custom("".to_string());
    let mut rank_white = Rank::Custom("".to_string());
    let mut komi = None;
    let mut result = GameResult::Unknown("".to_string());
    let mut rules = None;

    // Extract metadata from root node
    for prop in &game[0].properties {
        match prop {
            go::Prop::EV(e) => event = e.text.to_string(),
            go::Prop::RO(r) => round = r.text.to_string(),
            go::Prop::PC(p) => location = p.text.to_string(),
            go::Prop::DT(d) => date = Some(parse_sgf_date(&d.text)),
            go::Prop::PB(p) => player_black = p.text.to_string(),
            go::Prop::PW(p) => player_white = p.text.to_string(),
            go::Prop::BR(r) => rank_black = parse_rank(&r.text),
            go::Prop::WR(r) => rank_white = parse_rank(&r.text),
            go::Prop::KM(k) => komi = parse_komi(&k.to_string()),
            go::Prop::RE(r) => result = parse_sgf_result(&r.text),
            go::Prop::RU(r) => rules = Some(parse_rules(&r.text)),
            _ => {}
        }
    }

    if has_multiple_players(&player_black) || has_multiple_players(&player_white) {
        return Err("Player name indicates multiple players".into());
    }

    if let Some(go::Prop::SZ(size)) = game[0]
        .properties
        .iter()
        .find(|p| matches!(p, go::Prop::SZ(_)))
    {
        if *size != (BOARD_SIZE, BOARD_SIZE) {
            return Err(
                format!("Got non-{BOARD_SIZE:?}x{BOARD_SIZE:?} board size: {size:?}").into(),
            );
        }
    }

    for node in game[0].main_variation() {
        for props in &node.properties {
            match props {
                go::Prop::W(go::Move::Move(point)) => {
                    if point.x >= BOARD_SIZE || point.y >= BOARD_SIZE {
                        println!(
                            "Skipping move greater than board size {BOARD_SIZE:?}x{BOARD_SIZE:?}, {point:?} in file: {path:?}"
                        );
                        break;
                    }
                    moves.push(Placement {
                        color: Color::White,
                        point: Point {
                            x: point.x,
                            y: point.y,
                        },
                    });
                    break;
                }
                go::Prop::B(go::Move::Move(point)) => {
                    if point.x >= BOARD_SIZE || point.y >= BOARD_SIZE {
                        println!(
                            "Skipping move greater than board size {BOARD_SIZE:?}x{BOARD_SIZE:?}, {point:?} in file: {path:?}"
                        );
                        break;
                    }
                    moves.push(Placement {
                        color: Color::Black,
                        point: Point {
                            x: point.x,
                            y: point.y,
                        },
                    });
                    break;
                }
                go::Prop::AB(points) => {
                    for point in points {
                        if point.x >= BOARD_SIZE || point.y >= BOARD_SIZE {
                            println!(
                                "Skipping handicap placement greater than board size {BOARD_SIZE:?}x{BOARD_SIZE:?}, {point:?} in file: {path:?}"
                            );
                            continue;
                        }
                        moves.push(Placement {
                            color: Color::Black,
                            point: Point {
                                x: point.x,
                                y: point.y,
                            },
                        });
                    }
                }
                go::Prop::AW(points) => {
                    for point in points {
                        if point.x >= BOARD_SIZE || point.y >= BOARD_SIZE {
                            println!(
                                "Skipping handicap placement greater than board size {BOARD_SIZE:?}x{BOARD_SIZE:?}, {point:?} in file: {path:?}"
                            );
                            continue;
                        }
                        moves.push(Placement {
                            color: Color::White,
                            point: Point {
                                x: point.x,
                                y: point.y,
                            },
                        });
                    }
                }
                _ => {}
            }
        }
    }

    if moves.is_empty() {
        return Err("Game has no moves".into());
    } else if moves.len() < 5 {
        return Err("Game has less than 5 moves".into());
    }

    Ok((
        Game {
            event,
            round,
            location,
            date,
            player_black: Player::Unknown(player_black.clone()),
            player_white: Player::Unknown(player_white.clone()),
            rank_black,
            rank_white,
            komi,
            result,
            rules,
            moves,
            captures: HashMap::new(),
        },
        player_black,
        player_white,
    ))
}
