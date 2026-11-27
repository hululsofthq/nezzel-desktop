//! Developer-only: runs the hidden catch on a list of pages and writes what each one gave.
//!
//! ```sh
//! cargo run --features catch-matrix --example catch_matrix -j 4 -- \
//!   --links <outside the repo>/catch-matrix.tsv --out <dir>/results.json [--only word]
//! ```
//! The list is tab-separated: `category`, `url`, `expect`, free text. It is never committed.
fn main() {
  nezzel_lib::catch::matrix::main();
}
