# lua-src parchado para Flint

Copia de [lua-src 551.0.1](https://crates.io/crates/lua-src/551.0.1), el
crate que compila Lua para `mlua` (con el feature `vendored`). El
`[patch.crates-io]` de `Cargo.toml` hace que Cargo use esta copia en vez de
la de crates.io.

## Por qué

`add_files_by_ext` (en `src/lib.rs`) junta los `.c` de Lua con
`fs::read_dir` y los compila en el orden en que llegan. Ese orden depende
del sistema de archivos: en ext4 sale de un hash con una semilla distinta en
cada disco, y en tmpfs del orden en que se crearon los archivos. Es también
el orden de los objetos en `liblua5.4.a` y, por lo tanto, cómo el enlazador
arma el binario. Así, el mismo commit daba otro binario según el disco donde
se compilara, y el `.deb` publicado no se podía reproducir.

Comprobado en tmpfs, creando los `.c` en un orden y en el inverso: sin el
parche salen dos sha256 distintos; con el parche, el mismo.

## Qué cambia respecto del original

- `src/lib.rs`: la lista de archivos se ordena antes de compilarla. Es el
  único cambio de código.
- Solo trae `lua-5.4.8`, que es la versión que usa Flint. Las demás
  (5.1, 5.2, 5.3 y 5.5) no están.
- `Cargo.toml`: sin el `[[test]]` ni las `dev-dependencies`, porque los
  tests del crate no vienen en la copia.

## Cuándo se puede quitar

Cuando lua-src publique una versión que ordene los archivos por su cuenta:
se borra este directorio y el `[patch.crates-io]`, y se sube `mlua` hasta
la versión que use esa lua-src.
