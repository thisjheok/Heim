<div align="center">
  <img src="icon.png" alt="Heim app icon" width="128" />
</div>

<p align="center"><strong>English</strong> | <a href="README.ko.md">한국어</a></p>

# Heim

Heim is a desktop workspace for planning and building software with an AI agent. It keeps a project's architecture map, design decisions, development tickets, and agent activity together so you can move from an approved design to implementation with a clear, reviewable scope.

## Features

- Create and edit a project's architecture map, with typed components and relationships.
- Ask the AI agent to create or revise a design draft; review and explicitly approve the draft before it becomes the confirmed map.
- Plan work as features split into tickets (by the AI on request, or by hand). Each ticket has a goal, observable completion criteria, and the map nodes it touches; the next ticket is simply the first open one in order.
- Run development through the Codex CLI, following its configured permissions and approval policy.
- Review the agent's report, changed files, checks, and available run evidence before recording a review decision.
- Store project maps, drafts, features, tickets, reports, and reviews in a local SQLite database.

Heim does not claim to sandbox the agent or force approval for every command. Agent access and approvals follow the installed Codex CLI's configuration.

## Requirements

- Node.js and npm
- Rust toolchain (stable) and Cargo
- Tauri v2 platform prerequisites for your operating system ([Tauri prerequisites](https://v2.tauri.app/start/prerequisites/))
- Codex CLI installed and available on `PATH` to use AI-agent features

## Run locally

The Tauri desktop app is in `app/`.

```sh
cd app
npm install
npm run tauri:dev
```

To run the frontend in a browser during UI development:

```sh
cd app
npm install
npm run dev
```

The browser frontend does not provide the native Tauri backend features.

## Checks and production build

Run these from `app/`:

```sh
npm run lint
npm run build
npm run tauri:build
```

`build` type-checks the frontend and creates its production assets. `tauri:build` packages the desktop application and runs the frontend build first.

## Project layout

```text
app/
  src/            React and TypeScript user interface
  src/data/       Map-related frontend data and types
  src-tauri/      Tauri desktop backend, agent integration, and SQLite persistence
icon.png          Project icon used above
```

## License

No license has been specified yet.
