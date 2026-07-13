# Git CMS

Rust backend for a GitHub-backed CMS. It exposes allowlisted UTF-8 content,
creates a draft pull request per editing session, and pushes session commits
using the authenticated GitHub user's GitHub App user token.

## Run

1. Copy `cms.example.toml` to `cms.toml` and set the repository/content values.
2. Copy `.env.example` to `.env` and provide the GitHub App and CMS JWT secrets.
3. Register the GitHub App user authorization callback as
   `http://localhost:3000/auth/github/callback` (or your configured public URL).
4. Run `cargo run`.

## Example frontend

The `frontend` directory contains a small Vite-based editor. It handles the
GitHub sign-in callback, keeps the CMS JWT in browser local storage, and creates
a draft session automatically when the user first saves a file.

In a second terminal, run:

```sh
cd frontend
npm install
npm run dev
```

Open `http://localhost:5173`. The supplied `cms.example.toml` already uses
`http://localhost:5173/auth/callback` as its `frontend_callback_url`.

Start authorization at `GET /auth/github/start`. The callback redirects to
`server.frontend_callback_url?token=<cms-jwt>`; the frontend sends that token
as `Authorization: Bearer <cms-jwt>` to `/api/*`.

The app installation needs repository read access for startup cloning. Users
need repository write access for session branch pushes and pull request changes.
No GitHub token is saved in repository configuration; user credentials are held
only in process memory, so signing in again is required after a backend restart.
