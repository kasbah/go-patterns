use rayon::prelude::*;
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use go_patterns_common::baduk::{
    Game, GameResult, Placement, Player, Rank, Rules, SgfDate, all_rotations,
};

// Structure to track possible player ID aliases
#[derive(Debug, Hash, Eq, PartialEq)]
pub struct PossiblePlayerAlias {
    pub id1: i16,
    pub id2: i16,
}

#[derive(Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
struct PlayersAndDateKey {
    player_black: Option<i16>,
    player_white: Option<i16>,
    date: Option<SgfDate>,
    first_50_moves: Vec<Placement>,
}

pub fn find_duplicates(
    games_vec: Vec<(String, Game)>,
) -> (Vec<(String, Game)>, HashSet<PossiblePlayerAlias>) {
    let possible_aliases = Mutex::new(HashSet::new());
    let unique_games = Mutex::new(Vec::<(Vec<Placement>, (String, Game))>::new());

    println!("Removing duplicates from {} games...", games_vec.len());

    let processed_count = AtomicUsize::new(0);
    let total_games = games_vec.len();

    games_vec.into_par_iter().for_each(|(path, game)| {
        assert!(game.moves.len() > 10, "Not enough moves in game");

        let rotations = all_rotations(&game.moves);

        let mut is_duplicate = false;

        for position in rotations {
            let mut unique_games_guard = unique_games.lock().unwrap();
            if let Some((_existing_moves, (_existing_path, existing_game))) = unique_games_guard
                .iter_mut()
                .find(|(moves, _)| *moves == position)
            {
                is_duplicate = true;

                // Record possible aliases before merging
                if let (Player::Id(id1, _), Player::Id(id2, _)) =
                    (&existing_game.player_black, &game.player_black)
                {
                    if id1 != id2 {
                        possible_aliases
                            .lock()
                            .unwrap()
                            .insert(PossiblePlayerAlias {
                                id1: *id1,
                                id2: *id2,
                            });
                    }
                }
                if let (Player::Id(id1, _), Player::Id(id2, _)) =
                    (&existing_game.player_white, &game.player_white)
                {
                    if id1 != id2 {
                        possible_aliases
                            .lock()
                            .unwrap()
                            .insert(PossiblePlayerAlias {
                                id1: *id1,
                                id2: *id2,
                            });
                    }
                }

                merge_games(existing_game, &game);
                break;
            }
        }

        if !is_duplicate {
            unique_games
                .lock()
                .unwrap()
                .push((game.moves.clone(), (path, game.clone())));
        }

        let current = processed_count.fetch_add(1, Ordering::Relaxed) + 1;
        if current % 1000 == 0 || current == total_games {
            println!("Processed {current}/{total_games} games");
        }
    });

    let unique_games = unique_games.into_inner().unwrap();
    let possible_aliases = possible_aliases.into_inner().unwrap();

    println!("Found {} unique games", unique_games.len());

    println!("Second deduplication pass (players, date, first 50 moves)...");
    let games_by_date = Mutex::new(Vec::<(PlayersAndDateKey, (String, Game))>::new());

    let processed_count_second = AtomicUsize::new(0);
    let total_unique_games = unique_games.len();

    unique_games
        .into_par_iter()
        .for_each(|(_moves, (path, game))| {
            let first_50_moves: Vec<_> = game.moves.iter().take(50).cloned().collect();
            let player_black = match &game.player_black {
                Player::Id(id, ..) => Some(*id),
                _ => None,
            };
            let player_white = match &game.player_white {
                Player::Id(id, ..) => Some(*id),
                _ => None,
            };

            let key = PlayersAndDateKey {
                player_black,
                player_white,
                date: game.date.clone(),
                first_50_moves,
            };

            let mut games_by_date_guard = games_by_date.lock().unwrap();
            if let Some((_existing_key, (_existing_path, existing_game))) =
                games_by_date_guard.iter_mut().find(|(k, _)| *k == key)
            {
                merge_games(existing_game, &game);
            } else {
                games_by_date_guard.push((key, (path, game)));
            }

            let current = processed_count_second.fetch_add(1, Ordering::Relaxed) + 1;
            if current % 500 == 0 || current == total_unique_games {
                println!("Second pass: processed {current}/{total_unique_games} games");
            }
        });

    let games_by_date = games_by_date.into_inner().unwrap();

    // Convert back to the format expected by the rest of the code
    let final_unique_games: Vec<(String, Game)> = games_by_date
        .into_par_iter()
        .map(|(_key, (path, game))| (path, game))
        .collect();

    println!(
        "After second deduplication: {} unique games",
        final_unique_games.len()
    );

    (final_unique_games, possible_aliases)
}

fn merge_games(existing_game: &mut Game, new_game: &Game) {
    let merged_player_black = match (&existing_game.player_black, &new_game.player_black) {
        (Player::Id(id1, name1), Player::Id(id2, _name2)) => {
            Player::Id(if *id1 > 0 { *id1 } else { *id2 }, name1.clone())
        }
        (Player::Id(id, name), Player::Unknown(_)) => Player::Id(*id, name.clone()),
        (Player::Unknown(_), Player::Id(id, name)) => Player::Id(*id, name.clone()),
        (Player::Unknown(name1), Player::Unknown(name2)) => Player::Unknown(if !name1.is_empty() {
            name1.clone()
        } else {
            name2.clone()
        }),
    };

    let merged_player_white = match (&existing_game.player_white, &new_game.player_white) {
        (Player::Id(id1, name1), Player::Id(id2, _name2)) => {
            Player::Id(if *id1 > 0 { *id1 } else { *id2 }, name1.clone())
        }
        (Player::Id(id, name), Player::Unknown(_)) => Player::Id(*id, name.clone()),
        (Player::Unknown(_), Player::Id(id, name)) => Player::Id(*id, name.clone()),
        (Player::Unknown(name1), Player::Unknown(name2)) => Player::Unknown(if !name1.is_empty() {
            name1.clone()
        } else {
            name2.clone()
        }),
    };

    let merged_result = match (&existing_game.result, &new_game.result) {
        (GameResult::Unknown(_), new) => new.clone(),
        (existing, GameResult::Unknown(_)) => existing.clone(),
        (GameResult::Player(_, None, _), GameResult::Player(_, Some(_), _)) => {
            new_game.result.clone()
        }
        (GameResult::Player(_, Some(_), _), GameResult::Player(_, None, _)) => {
            existing_game.result.clone()
        }
        _ => existing_game.result.clone(),
    };

    let merged_rank_black = match (&new_game.rank_black, &existing_game.rank_black) {
        (Rank::Pro(_), _) => new_game.rank_black.clone(),
        (_, Rank::Pro(_)) => existing_game.rank_black.clone(),
        (Rank::Dan(_), _) => new_game.rank_black.clone(),
        (_, Rank::Dan(_)) => existing_game.rank_black.clone(),
        (Rank::Kyu(_), _) => new_game.rank_black.clone(),
        (_, Rank::Kyu(_)) => existing_game.rank_black.clone(),
        (Rank::Custom(s1), Rank::Custom(_)) if s1.is_empty() => existing_game.rank_black.clone(),
        _ => new_game.rank_black.clone(),
    };
    let merged_rank_white = match (&new_game.rank_white, &existing_game.rank_white) {
        (Rank::Pro(_), _) => new_game.rank_white.clone(),
        (_, Rank::Pro(_)) => existing_game.rank_white.clone(),
        (Rank::Dan(_), _) => new_game.rank_white.clone(),
        (_, Rank::Dan(_)) => existing_game.rank_white.clone(),
        (Rank::Kyu(_), _) => new_game.rank_white.clone(),
        (_, Rank::Kyu(_)) => existing_game.rank_white.clone(),
        (Rank::Custom(s1), Rank::Custom(_)) if s1.is_empty() => existing_game.rank_white.clone(),
        _ => new_game.rank_white.clone(),
    };

    let merged_location = match (&new_game.location, &existing_game.location) {
        (l1, _) if !l1.is_empty() => new_game.location.clone(),
        (_, l2) if !l2.is_empty() => existing_game.location.clone(),
        _ => new_game.location.clone(),
    };
    let merged_round = match (&new_game.round, &existing_game.round) {
        (r1, _) if !r1.is_empty() => new_game.round.clone(),
        (_, r2) if !r2.is_empty() => existing_game.round.clone(),
        _ => new_game.round.clone(),
    };
    let merged_event = match (&new_game.event, &existing_game.event) {
        (e1, _) if !e1.is_empty() => new_game.event.clone(),
        (_, e2) if !e2.is_empty() => existing_game.event.clone(),
        _ => new_game.event.clone(),
    };

    let merged_date = match (&new_game.date, &existing_game.date) {
        (Some(d1), Some(d2)) => {
            // If both dates exist, use the more precise one
            match (d1, d2) {
                (SgfDate::YearMonthDay(_, _, _), _) => new_game.date.clone(),
                (_, SgfDate::YearMonthDay(_, _, _)) => existing_game.date.clone(),
                (SgfDate::YearMonth(_, _), SgfDate::YearMonth(_, _)) => new_game.date.clone(),
                (SgfDate::YearMonth(_, _), _) => new_game.date.clone(),
                (_, SgfDate::YearMonth(_, _)) => existing_game.date.clone(),
                _ => new_game.date.clone(),
            }
        }
        (Some(_), None) => new_game.date.clone(),
        (None, Some(_)) => existing_game.date.clone(),
        _ => new_game.date.clone(),
    };
    let merged_rules = match (&new_game.rules, &existing_game.rules) {
        (Some(_), Some(Rules::Custom(_))) => new_game.rules.clone(),
        (Some(_), None) => new_game.rules.clone(),
        (None, _) => existing_game.rules.clone(),
        _ => new_game.rules.clone(),
    };

    if new_game.moves.len() > existing_game.moves.len() {
        existing_game.moves = new_game.moves.clone();
    }

    // Apply merged values
    existing_game.event = merged_event;
    existing_game.round = merged_round;
    existing_game.location = merged_location;
    existing_game.date = merged_date;
    existing_game.player_black = merged_player_black;
    existing_game.player_white = merged_player_white;
    existing_game.rank_black = merged_rank_black;
    existing_game.rank_white = merged_rank_white;
    existing_game.komi = new_game.komi;
    existing_game.result = merged_result;
    existing_game.rules = merged_rules;
}
