#!/bin/bash
# Testes: só com os fixtures (tests/fixtures); nada fora de pastas temporárias é tocado
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
echo "🧪 Teste — dados reais não serão modificados"
cargo test --manifest-path "$DIR/Cargo.toml" "$@"
