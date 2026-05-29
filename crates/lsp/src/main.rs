//! `eye-lsp` binary — stdio language server for `.eye` files.

use lsp_server::Connection;

fn main() -> anyhow::Result<()> {
  if std::env::var_os("EYE_LSP_LOG").is_some() {
    eprintln!("eye-lsp starting");
  }

  let (connection, io_threads) = Connection::stdio();
  eye_lsp::run(&connection)?;
  io_threads.join()?;
  Ok(())
}
