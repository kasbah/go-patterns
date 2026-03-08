# Go Pattern Search | [go-patterns.kaspar.systems](https://go-patterns.kaspar.systems)

[![screenshot](./screenshot.png)](https://go-patterns.kaspar.systems)

🚧 _Work in Progress_ 🚧

A pattern search for the ancient [game of Go](<https://en.wikipedia.org/wiki/Go_(game)>).

- Play / draw stone patterns on the board and find pro and high-ranked amateur games with similar positions
- Over 100,000 game records searched
- Filter by player names (normalized by aliases / romanization differences)

## Inspiration

This is my take on a patterns search idea that I first saw on [Waltheri's pattern search](https://ps.waltheri.net/). The main differences are:

- Fuzzier matching, e.g. the order of moves in your pattern is not important at all and both "around stones" and whole board patterns are taken into account at all times
- Player names are normalized and can be filtered on
- Many different UI ideas
- Waltheri may have a better SGF game record database, ours is a combination of public domain SGF libraries we scrounged together
