#!/usr/bin/env bash
# Подписывает файлы каталога $1 minisign и выкладывает их в релиз $TAG на GitHub.
# Релиза ещё нет — создаёт его (первая готовая сборка), есть — докладывает файлы с заменой.
# Окружение: TAG, GH_TOKEN, GH_REPO, GITHUB_SHA; MINISIGN_SECRET_KEY — необязательно.
# Запуск: bash scripts/release-publish.sh <каталог>
set -euo pipefail

dir="$1"
shopt -s nullglob
files=("$dir"/*)
if [ ${#files[@]} -eq 0 ]; then
  echo "::error::В $dir нет файлов для релиза"
  exit 1
fi

if [ -n "${MINISIGN_SECRET_KEY:-}" ]; then
  if ! command -v minisign >/dev/null; then
    sudo apt-get update -qq && sudo apt-get install -y -qq minisign >/dev/null
  fi
  key="$(mktemp)"
  trap 'rm -f "$key"' EXIT
  (umask 077 && printf '%s\n' "$MINISIGN_SECRET_KEY" >"$key")
  # Подписи должны проверяться ключом, вшитым в LMQ, — иначе автообновление их отвергнет.
  pk=$(sed -n 's/.*DEFAULT_UPDATES_PUBKEY: &str = "\(.*\)";/\1/p' crates/lmq-core/src/config/settings.rs)
  for f in "${files[@]}"; do
    case "$f" in *.minisig) continue ;; esac
    minisign -S -s "$key" -m "$f" -t "LMQ $TAG $(basename "$f")"
    minisign -V -P "$pk" -m "$f" -q
  done
else
  echo "::warning::Секрет MINISIGN_SECRET_KEY не задан: файлы без подписей, LMQ покажет только ссылку и не установит обновление сам"
fi

files=("$dir"/*)
pre=""
case "$TAG" in *-*) pre="--prerelease" ;; esac
# Два задания могут прийти одновременно: проигравшее гонку за создание просто докладывает файлы.
if gh release view "$TAG" >/dev/null 2>&1 \
  || ! gh release create "$TAG" "${files[@]}" --target "$GITHUB_SHA" --title "LMQ ${TAG#v}" --generate-notes $pre; then
  gh release upload "$TAG" "${files[@]}" --clobber
fi
ls -l "$dir"
gh release view "$TAG" --json url,assets --jq '.url, (.assets[].name)'
