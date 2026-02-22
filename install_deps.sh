curl -fsSL https://bun.sh/install | bash
export PATH="$HOME/.bun/bin:$PATH"
cd frontend
bun install
bun run build