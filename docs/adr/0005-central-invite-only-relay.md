# 0005. One central, invite-only relay, multi-tenant by design

- **Status:** Accepted
- **Date:** 2026-10-11

## Context

Desktop apps can't safely hold Turso Platform credentials, and children sign in with their father's email plus a key, so a small server is required. Papa starts as a project for one family and a few friends, but may open to others later. Self-hosting per father adds a lot of friction for non-technical users.

## Decision

- Run one central `papa-relay`, operated by the project owner.
- Sign-up requires an invite code, minted with an admin command.
- Build it multi-tenant from day one: every table is keyed by account, the API is versioned (`/v1`), its types live in `papa-proto`, secrets come only from environment variables, and it has rate limiting and structured logs.
- The relay URL is configurable and is embedded in each child's access key, so self-hosting stays possible later.

## Consequences

- Fathers get zero setup. The owner pays for and operates a small server.
- Before inviting people outside the family, the project needs a privacy policy and account export and deletion.
- Opening sign-ups later is a configuration change, not a redesign.
