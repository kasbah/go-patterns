extern crate cfg_if;
extern crate wasm_bindgen;

mod utils;

use cfg_if::cfg_if;
use go_patterns_common::baduk::{
    Color, Game, GameResult, Placement, Player, Point, Rank, Rotation, Rules, SgfDate, check_empty,
    check_within_one_quadrant, get_mirrored, get_rotated, get_rotations, get_surrounding_points,
    match_game, switch_colors, unpack_games,
};
use indexmap::IndexMap;
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::js_sys::Uint8Array;

cfg_if! {
    if #[cfg(feature = "wee_alloc")] {
        extern crate wee_alloc;
        #[global_allocator]
        static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
    }
}

#[wasm_bindgen]
extern "C" {
    fn alert(s: &str);
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

#[wasm_bindgen]
pub struct WasmSearch {
    game_data: IndexMap<String, Game>,
    position_cache: LruCache<Vec<Placement>, Vec<SearchResult>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    path: String,
    score: i16,
    last_move_matched: usize,
    rotation: u8,                      // 0: no rotation, 1-3: rotation index
    is_inverted: bool,                 // whether the colors were inverted
    is_mirrored: bool,                 // whether the position was mirrored
    all_empty_correctly_within: u8, // distance from moves where all surrounding points are correctly empty
    moves: Vec<Placement>,          // the actual game moves
    moves_transformed: Vec<Placement>, // the moves rotated and/or mirrored
    // Game metadata
    event: String,
    round: String,
    location: String,
    date: Option<SgfDate>,
    player_black: Player,
    player_white: Player,
    rank_black: Rank,
    rank_white: Rank,
    komi: Option<f32>,
    rules: Option<Rules>,
    result: GameResult,
}

/// Filter for matching players by ID and optionally by color
///
/// # Fields
/// * `player_id` - The ID of the player to match
/// * `color` - Optional color constraint:
///   - `None` - Match player regardless of color (black or white)
///   - `Some(Color::Black)` - Only match when player is playing black
///   - `Some(Color::White)` - Only match when player is playing white
#[derive(Serialize, Deserialize, Clone)]
pub struct PlayerFilter {
    player_id: i16,
    color: Option<Color>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct NextMove {
    point: Point,
    game_count: usize,
}

fn get_moves_rotation(query_rotation: &Rotation) -> Rotation {
    // rotating the moves the opposite to the query position
    match query_rotation {
        Rotation::Degrees90 => Rotation::Degrees270,
        Rotation::Degrees270 => Rotation::Degrees90,
        _ => *query_rotation,
    }
}

fn get_rotation_index(r: &Rotation) -> u8 {
    match r {
        Rotation::Degrees90 => 1,
        Rotation::Degrees180 => 2,
        Rotation::Degrees270 => 3,
    }
}

fn get_next_moves(
    results: &[SearchResult],
    position: &[Placement],
    next_color: Color,
) -> Vec<NextMove> {
    let mut next_moves_map: HashMap<Placement, (usize, usize)> = HashMap::new();
    let moves_ahead = 2;
    for result in results {
        let mut mult: usize = if result.last_move_matched == position.len() - 1 {
            100
        } else {
            1
        };
        mult *= result.all_empty_correctly_within as usize;
        if mult > 0 {
            for i in 1..=moves_ahead {
                if let Some(move_) = result.moves_transformed.get(result.last_move_matched + i) {
                    if !position.iter().any(|m| m.point == move_.point) {
                        let mut move_ = *move_;
                        if result.is_inverted {
                            move_.color = if move_.color == Color::White {
                                Color::Black
                            } else {
                                Color::White
                            };
                        }
                        if let Some((score, count)) = next_moves_map.get(&move_) {
                            next_moves_map
                                .insert(move_, (score + mult + moves_ahead - i, *count + 1));
                        } else {
                            next_moves_map.insert(move_, (mult + moves_ahead - i, 1));
                        }
                    }
                }
            }
        }
    }

    let next_placements = next_moves_map.iter().collect::<Vec<_>>();
    let mut next_moves = next_placements
        .into_iter()
        .filter(|(m, _)| m.color == next_color)
        .collect::<Vec<_>>();

    next_moves.sort_by(|a, b| b.1.cmp(a.1));
    next_moves
        .into_iter()
        .map(|(m, (_, count))| NextMove {
            point: m.point,
            game_count: *count,
        })
        .filter(|next_move| next_move.game_count >= 50)
        .collect()
}

#[derive(Serialize, Deserialize)]
struct WasmSearchReturn {
    num_results: usize,
    next_moves: Vec<NextMove>,
    results: Vec<SearchResult>,
    total_pages: usize,
    current_page: usize,
    player_counts: HashMap<i16, usize>, // player_id -> count of games
}

#[wasm_bindgen]
#[derive(Serialize, Deserialize, Clone, Eq, PartialEq)]
pub enum SortBy {
    BestMatch,
    LeastMoves,
}

#[wasm_bindgen]
impl WasmSearch {
    #[wasm_bindgen(constructor)]
    pub fn new() -> WasmSearch {
        let packed = include_bytes!("games.pack");
        let game_data = unpack_games(packed);
        let position_cache = LruCache::new(std::num::NonZeroUsize::new(1000).unwrap());

        Self {
            game_data,
            position_cache,
        }
    }

    /// Search for games matching the given position with optional player filtering
    ///
    /// # Parameters
    /// * `position` - JSON-encoded Vec<Placement> representing the board position
    /// * `next_color` - Color for next move (0 = Black, 1 = White)
    /// * `page` - Page number for pagination (0-based)
    /// * `page_size` - Number of results per page
    /// * `player_filters_json` - JSON-encoded Vec<PlayerFilter> for filtering by players
    ///   Pass empty array `[]` for no filtering, or array of PlayerFilter objects
    ///   to require ALL specified players to be present in matching games
    ///
    /// # Example usage of player_filters_json:
    /// ```
    /// // No filtering - matches all games
    /// []
    ///
    /// // Filter for games containing player ID 123 (any color)
    /// [{"player_id": 123, "color": null}]
    ///
    /// // Filter for games where player ID 123 plays black
    /// [{"player_id": 123, "color": "Black"}]
    ///
    /// // Filter for games where player ID 123 plays white
    /// [{"player_id": 123, "color": "White"}]
    ///
    /// // Filter for games containing both player ID 123 (any color) and player ID 456 as white
    /// [{"player_id": 123, "color": null}, {"player_id": 456, "color": "White"}]
    /// ```
    #[wasm_bindgen]
    pub async fn search(
        &mut self,
        position: Uint8Array,
        next_color: u8,
        page: usize,
        page_size: usize,
        player_filters_json: Uint8Array,
        sort_by: SortBy,
    ) -> Uint8Array {
        let position_buf: Vec<u8> = position.to_vec();
        let position_decoded: Vec<Placement> = serde_json::from_slice(position_buf.as_slice())
            .expect("Failed to deserialize position");

        let player_filters_buf: Vec<u8> = player_filters_json.to_vec();
        let player_filters: Vec<PlayerFilter> =
            serde_json::from_slice(player_filters_buf.as_slice())
                .expect("Failed to deserialize player filters");

        let mut results = self.match_position(&position_decoded);

        if sort_by == SortBy::LeastMoves {
            results.sort_by(|a, b| a.last_move_matched.cmp(&b.last_move_matched));
        }

        // Filter results by player filters if provided (empty array means no filter)
        // Games must contain ALL selected players with specified colors
        if !player_filters.is_empty() {
            results.retain(|result| {
                player_filters.iter().all(|filter| {
                    let player_id = filter.player_id;
                    match filter.color {
                        None => {
                            // Any color - check both black and white
                            matches!(result.player_black, Player::Id(id, _) if id == player_id)
                                || matches!(result.player_white, Player::Id(id, _) if id == player_id)
                        }
                        Some(Color::Black) => {
                            // Black only
                            matches!(result.player_black, Player::Id(id, _) if id == player_id)
                        }
                        Some(Color::White) => {
                            // White only
                            matches!(result.player_white, Player::Id(id, _) if id == player_id)
                        }
                    }
                })
            });
        }

        let next_color = if next_color == 0 {
            Color::Black
        } else {
            Color::White
        };
        let next_moves = get_next_moves(&results, &position_decoded, next_color);

        let num_results = results.len();
        let total_pages = num_results.div_ceil(page_size);
        let current_page = page.min(total_pages.saturating_sub(1));
        // Aggregate player counts from all results, excluding filtered players
        let mut player_counts: HashMap<i16, usize> = HashMap::new();
        let filtered_player_ids: Vec<i16> = player_filters.iter().map(|f| f.player_id).collect();
        for result in &results {
            // Count black player (exclude if it's one of the filtered players)
            if let Player::Id(player_id, _) = &result.player_black {
                if !filtered_player_ids.contains(player_id) {
                    *player_counts.entry(*player_id).or_insert(0) += 1;
                }
            }
            // Count white player (exclude if it's one of the filtered players)
            if let Player::Id(player_id, _) = &result.player_white {
                if !filtered_player_ids.contains(player_id) {
                    *player_counts.entry(*player_id).or_insert(0) += 1;
                }
            }
        }

        let start_idx = current_page * page_size;
        let end_idx = (start_idx + page_size).min(num_results);

        let ret = WasmSearchReturn {
            num_results,
            next_moves: next_moves[0..next_moves.len().min(9)].to_vec(),
            results: results[start_idx..end_idx].to_vec(),
            total_pages,
            current_page,
            player_counts,
        };

        let results_buf: Vec<u8> = serde_json::to_vec(&ret).expect("Failed to serialize results");
        Uint8Array::from(results_buf.as_slice())
    }

    /// Get a SearchResult by its path, rotation, and mirroring. Returns the SearchResult as a JSON Uint8Array, or an empty array if not found.
    ///
    /// # Arguments
    /// * `path` - The game path
    /// * `rotation` - 0: none, 1: 90°, 2: 180°, 3: 270°
    /// * `is_mirrored` - Whether to mirror the moves before rotation
    #[wasm_bindgen]
    pub fn get_search_result_by_path(
        &self,
        path: &str,
        rotation: u8,
        is_mirrored: bool,
    ) -> Uint8Array {
        if let Some(game) = self.game_data.get(path) {
            let moves_transformed = if is_mirrored {
                get_mirrored(&game.moves)
            } else {
                game.moves.clone()
            };
            let moves_transformed = match rotation {
                1 => get_rotated(&moves_transformed, &Rotation::Degrees90),
                2 => get_rotated(&moves_transformed, &Rotation::Degrees180),
                3 => get_rotated(&moves_transformed, &Rotation::Degrees270),
                _ => moves_transformed,
            };
            let result = SearchResult {
                path: path.to_string(),
                score: 0,
                last_move_matched: 0,
                rotation,
                is_inverted: false,
                is_mirrored,
                all_empty_correctly_within: 0,
                moves: game.moves.clone(),
                moves_transformed,
                event: game.event.clone(),
                round: game.round.clone(),
                location: game.location.clone(),
                date: game.date.clone(),
                player_black: game.player_black.clone(),
                player_white: game.player_white.clone(),
                rank_black: game.rank_black.clone(),
                rank_white: game.rank_white.clone(),
                komi: game.komi,
                rules: game.rules.clone(),
                result: game.result.clone(),
            };
            let result_json =
                serde_json::to_vec(&result).expect("Failed to serialize SearchResult");
            Uint8Array::from(result_json.as_slice())
        } else {
            Uint8Array::new(&wasm_bindgen::JsValue::NULL)
        }
    }

    fn match_position(&mut self, position: &[Placement]) -> Vec<SearchResult> {
        if let Some(results) = self.position_cache.get(&position.to_vec()) {
            return results.clone();
        }
        if position.is_empty() {
            let mut results = Vec::new();
            for (path, game) in &self.game_data {
                results.push(SearchResult {
                    path: path.clone(),
                    score: 0,
                    last_move_matched: 0,
                    rotation: 0,
                    is_inverted: false,
                    is_mirrored: false,
                    all_empty_correctly_within: 0,
                    moves: game.moves.clone(),
                    moves_transformed: game.moves.clone(),
                    event: game.event.clone(),
                    round: game.round.clone(),
                    location: game.location.clone(),
                    date: game.date.clone(),
                    player_black: game.player_black.clone(),
                    player_white: game.player_white.clone(),
                    rank_black: game.rank_black.clone(),
                    rank_white: game.rank_white.clone(),
                    komi: game.komi,
                    rules: game.rules.clone(),
                    result: game.result.clone(),
                });
            }
            self.position_cache.put(position.to_vec(), results.clone());
            return results;
        }
        let mut results = Vec::new();
        let rotations = get_rotations(position);
        let inverse = switch_colors(position);
        let inverse_rotations = get_rotations(&inverse);
        let mirrored = get_mirrored(position);
        let mirrored_rotations = get_rotations(&mirrored);
        let mirrored_inverse = get_mirrored(&inverse);
        let mirrored_inverse_rotations = get_rotations(&mirrored_inverse);
        let is_within_one_quadrant = check_within_one_quadrant(position);

        for (path, game) in &self.game_data {
            // Original position
            let mut matched = match_game(position, &game.moves);
            if let Some(last_move_matched) = matched {
                results.push(SearchResult {
                    path: path.clone(),
                    score: 100,
                    last_move_matched,
                    rotation: 0,
                    is_inverted: false,
                    is_mirrored: false,
                    all_empty_correctly_within: 0,
                    moves: game.moves.clone(),
                    moves_transformed: game.moves.clone(),
                    event: game.event.clone(),
                    round: game.round.clone(),
                    location: game.location.clone(),
                    date: game.date.clone(),
                    player_black: game.player_black.clone(),
                    player_white: game.player_white.clone(),
                    rank_black: game.rank_black.clone(),
                    rank_white: game.rank_white.clone(),
                    komi: game.komi,
                    rules: game.rules.clone(),
                    result: game.result.clone(),
                });
                continue;
            }

            // Original rotations
            for (r, rotated_position) in rotations.clone() {
                matched = match_game(&rotated_position, &game.moves);
                if let Some(last_move_matched) = matched {
                    let moves_rotation = get_moves_rotation(&r);
                    results.push(SearchResult {
                        path: path.clone(),
                        score: 99,
                        last_move_matched,
                        rotation: get_rotation_index(&r),
                        is_inverted: false,
                        is_mirrored: false,
                        all_empty_correctly_within: 0,
                        moves: game.moves.clone(),
                        moves_transformed: get_rotated(&game.moves, &moves_rotation),
                        event: game.event.clone(),
                        round: game.round.clone(),
                        location: game.location.clone(),
                        date: game.date.clone(),
                        player_black: game.player_black.clone(),
                        player_white: game.player_white.clone(),
                        rank_black: game.rank_black.clone(),
                        rank_white: game.rank_white.clone(),
                        komi: game.komi,
                        rules: game.rules.clone(),
                        result: game.result.clone(),
                    });
                    break;
                }
            }

            {
                let mirrored_score = if is_within_one_quadrant { 100 } else { 10 };
                // Mirrored position
                if matched.is_none() {
                    matched = match_game(&mirrored, &game.moves);
                    if let Some(last_move_matched) = matched {
                        results.push(SearchResult {
                            path: path.clone(),
                            score: mirrored_score,
                            last_move_matched,
                            rotation: 0,
                            is_inverted: false,
                            is_mirrored: true,
                            all_empty_correctly_within: 0,
                            moves: game.moves.clone(),
                            moves_transformed: get_mirrored(&game.moves),
                            event: game.event.clone(),
                            round: game.round.clone(),
                            location: game.location.clone(),
                            date: game.date.clone(),
                            player_black: game.player_black.clone(),
                            player_white: game.player_white.clone(),
                            rank_black: game.rank_black.clone(),
                            rank_white: game.rank_white.clone(),
                            komi: game.komi,
                            rules: game.rules.clone(),
                            result: game.result.clone(),
                        });
                        continue;
                    }
                }

                // Mirrored rotations
                if matched.is_none() {
                    for (r, rotated_position) in mirrored_rotations.clone() {
                        matched = match_game(&rotated_position, &game.moves);
                        if let Some(last_move_matched) = matched {
                            results.push(SearchResult {
                                path: path.clone(),
                                score: mirrored_score - 1,
                                last_move_matched,
                                rotation: get_rotation_index(&r),
                                is_inverted: false,
                                is_mirrored: true,
                                all_empty_correctly_within: 0,
                                moves: game.moves.clone(),
                                moves_transformed: get_rotated(&get_mirrored(&game.moves), &r),
                                event: game.event.clone(),
                                round: game.round.clone(),
                                location: game.location.clone(),
                                date: game.date.clone(),
                                player_black: game.player_black.clone(),
                                player_white: game.player_white.clone(),
                                rank_black: game.rank_black.clone(),
                                rank_white: game.rank_white.clone(),
                                komi: game.komi,
                                rules: game.rules.clone(),
                                result: game.result.clone(),
                            });
                            break;
                        }
                    }
                }
            }

            // Inverse colors position
            if matched.is_none() {
                matched = match_game(&inverse, &game.moves);
                if let Some(last_move_matched) = matched {
                    results.push(SearchResult {
                        path: path.clone(),
                        score: 90,
                        last_move_matched,
                        rotation: 0,
                        is_inverted: true,
                        is_mirrored: false,
                        all_empty_correctly_within: 0,
                        moves: game.moves.clone(),
                        moves_transformed: game.moves.clone(),
                        event: game.event.clone(),
                        round: game.round.clone(),
                        location: game.location.clone(),
                        date: game.date.clone(),
                        player_black: game.player_black.clone(),
                        player_white: game.player_white.clone(),
                        rank_black: game.rank_black.clone(),
                        rank_white: game.rank_white.clone(),
                        komi: game.komi,
                        rules: game.rules.clone(),
                        result: game.result.clone(),
                    });
                    continue;
                }
            }

            // Inverse rotations
            if matched.is_none() {
                for (r, rotated_position) in inverse_rotations.clone() {
                    matched = match_game(&rotated_position, &game.moves);
                    if let Some(last_move_matched) = matched {
                        let moves_rotation = get_moves_rotation(&r);
                        results.push(SearchResult {
                            path: path.clone(),
                            score: 89,
                            last_move_matched,
                            rotation: get_rotation_index(&r),
                            is_inverted: true,
                            is_mirrored: false,
                            all_empty_correctly_within: 0,
                            moves: game.moves.clone(),
                            moves_transformed: get_rotated(&game.moves, &moves_rotation),
                            event: game.event.clone(),
                            round: game.round.clone(),
                            location: game.location.clone(),
                            date: game.date.clone(),
                            player_black: game.player_black.clone(),
                            player_white: game.player_white.clone(),
                            rank_black: game.rank_black.clone(),
                            rank_white: game.rank_white.clone(),
                            komi: game.komi,
                            rules: game.rules.clone(),
                            result: game.result.clone(),
                        });
                        break;
                    }
                }
            }

            {
                let mirrored_score = if is_within_one_quadrant { 90 } else { 9 };
                // Mirrored inverse position
                if matched.is_none() {
                    matched = match_game(&mirrored_inverse, &game.moves);
                    if let Some(last_move_matched) = matched {
                        results.push(SearchResult {
                            path: path.clone(),
                            score: mirrored_score,
                            last_move_matched,
                            rotation: 0,
                            is_inverted: true,
                            is_mirrored: true,
                            all_empty_correctly_within: 0,
                            moves: game.moves.clone(),
                            moves_transformed: get_mirrored(&game.moves),
                            event: game.event.clone(),
                            round: game.round.clone(),
                            location: game.location.clone(),
                            date: game.date.clone(),
                            player_black: game.player_black.clone(),
                            player_white: game.player_white.clone(),
                            rank_black: game.rank_black.clone(),
                            rank_white: game.rank_white.clone(),
                            komi: game.komi,
                            rules: game.rules.clone(),
                            result: game.result.clone(),
                        });
                        continue;
                    }
                }

                // Mirrored inverse rotations
                if matched.is_none() {
                    for (r, rotated_position) in mirrored_inverse_rotations.clone() {
                        matched = match_game(&rotated_position, &game.moves);
                        if let Some(last_move_matched) = matched {
                            results.push(SearchResult {
                                path: path.clone(),
                                score: mirrored_score - 1,
                                last_move_matched,
                                rotation: get_rotation_index(&r),
                                is_inverted: true,
                                is_mirrored: true,
                                all_empty_correctly_within: 0,
                                moves: game.moves.clone(),
                                moves_transformed: get_rotated(&get_mirrored(&game.moves), &r),
                                event: game.event.clone(),
                                round: game.round.clone(),
                                location: game.location.clone(),
                                date: game.date.clone(),
                                player_black: game.player_black.clone(),
                                player_white: game.player_white.clone(),
                                rank_black: game.rank_black.clone(),
                                rank_white: game.rank_white.clone(),
                                komi: game.komi,
                                rules: game.rules.clone(),
                                result: game.result.clone(),
                            });
                            break;
                        }
                    }
                }
            }
        }
        for result in &mut results {
            let truncated_moves = &result.moves_transformed[..result.last_move_matched];
            let mut checked = Vec::new();
            let mut all_empty_correctly_within = 0;
            let captures: Vec<Point> = self
                .game_data
                .get(&result.path)
                .expect("Inconsistent game data")
                .captures
                .iter()
                .filter(|(move_number, _)| move_number <= &&result.last_move_matched)
                .flat_map(|(_, cs)| cs.iter().map(|c| c.point))
                .collect::<Vec<_>>();

            for i in 1..=3 {
                let mut all_empty = true;
                for placement in position {
                    let mut surrounding = get_surrounding_points(&placement.point, i);
                    surrounding = surrounding
                        .iter()
                        .filter(|p| !position.iter().any(|m| m.point == **p))
                        .filter(|p| !checked.contains(*p))
                        .filter(|p| !captures.contains(*p))
                        .cloned()
                        .collect();
                    checked.extend(surrounding.iter().cloned());
                    if check_empty(&surrounding, truncated_moves) {
                        result.score += i as i16 * 3;
                    } else {
                        all_empty = false;
                        break;
                    }
                }
                if all_empty && (all_empty_correctly_within == i - 1) {
                    all_empty_correctly_within += 1;
                }
            }
            result.all_empty_correctly_within = all_empty_correctly_within;
            // all being empty around the position we are searching is very important, hence we
            // multiply the score
            result.score *= 1 + all_empty_correctly_within as i16;
        }

        for result in &mut results {
            result.score -= result.last_move_matched as i16;
        }

        results.sort_by(|a, b| b.score.cmp(&a.score));

        self.position_cache.put(position.to_vec(), results.clone());

        results
    }
}

impl Default for WasmSearch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instantiate() {
        let wasm_search = WasmSearch::new();
        assert!(!wasm_search.game_data.is_empty());
    }
}
