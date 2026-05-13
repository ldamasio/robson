# Model Context Protocol (MCP) for Robson v2

This guide explains how to configure and use MCP servers with Claude Code for enhanced development experience.

## What is MCP?

**Model Context Protocol (MCP)** enables Claude Code to integrate with external tools and services:

- **GitHub**: Search code, create issues/PRs, review commits
- **PostgreSQL**: Inspect schema, test queries, validate migrations
- **Sentry**: Debug production errors, analyze stack traces
- **Filesystem**: Enhanced file access and search

## Benefits for Robson v2 Development

### GitHub Integration
✅ Search for similar patterns across the entire codebase
✅ Create issues for discovered bugs or improvements
✅ Review PRs and check recent changes
✅ Find references to specific functions/types

**Example**: "Search GitHub for all uses of `rust_decimal::Decimal` in the codebase"

### PostgreSQL Integration
✅ Inspect database schema before writing SQLx queries
✅ Test queries in REPL before adding to repository code
✅ Analyze query performance with EXPLAIN
✅ Validate migration scripts before running

**Example**: "Show me the schema for the `orders` table and suggest an index for symbol+status queries"

### Sentry Integration
✅ Debug production errors with full stack traces
✅ Correlate errors with recent code changes
✅ Identify error patterns and frequency
✅ Create fixes based on actual production failures

**Example**: "Show me the most recent panic in robsond and help me fix it"

### Filesystem Integration
✅ Quick file access without explicit Read commands
✅ Search across multiple files efficiently
✅ List directory contents recursively

**Example**: "Find all files that import `robson_domain::Order`"

## Setup Instructions

### 1. Copy Example Configuration

```bash
cd /home/psyctl/apps/robson/v2
cp .mcp.example.json .mcp.json
```

### 2. Configure Environment Variables

Create `.env.mcp` file (excluded from git):

```bash
# GitHub (required for GitHub MCP)
export GITHUB_TOKEN="ghp_your_token_here"

# PostgreSQL (required for Postgres MCP)
export ROBSON_DATABASE_URL="postgresql://user:password@localhost:5432/robson_v2"

# Sentry (required for Sentry MCP)
export SENTRY_AUTH_TOKEN="your_sentry_token_here"
```

Load environment before starting Claude Code:

```bash
source .env.mcp
claude-code
```

### 3. Install MCP Servers

MCP servers are automatically installed on first use (via `npx`). To pre-install:

```bash
# GitHub
npx -y @modelcontextprotocol/server-github

# PostgreSQL
npx -y @modelcontextprotocol/server-postgres

# Sentry
npx -y @modelcontextprotocol/server-sentry

# Filesystem
npx -y @modelcontextprotocol/server-filesystem
```

### 4. Verify Configuration

Start Claude Code and check MCP status:

```bash
claude-code --mcp-status
```

You should see all configured servers listed as "Ready".

## Configuration Details

### GitHub MCP

**Required Permissions**:
- `repo` scope (for private repos)
- `read:org` (if repo is in organization)

**Create Token**:
1. Go to https://github.com/settings/tokens
2. Click "Generate new token (classic)"
3. Select scopes: `repo`, `read:org`
4. Copy token to `GITHUB_TOKEN` environment variable

**Configuration**:
```json
{
  "github": {
    "env": {
      "GITHUB_TOKEN": "${GITHUB_TOKEN}",
      "GITHUB_OWNER": "ldamasio",
      "GITHUB_REPO": "robson"
    }
  }
}
```

### PostgreSQL MCP

**Required**:
- PostgreSQL database running locally or remotely
- Connection string with appropriate permissions

**Create Development Database**:
```bash
# Create database
createdb robson_v2_dev

# Run migrations (once robson-store is implemented)
sqlx migrate run
```

**Configuration**:
```json
{
  "postgres": {
    "env": {
      "POSTGRES_CONNECTION_STRING": "${ROBSON_DATABASE_URL}"
    }
  }
}
```

**Security Warning**:
⚠️ Only use development/staging databases. Never point MCP to production databases with write access!

### Sentry MCP

**Required Permissions**:
- `project:read` (view errors)
- `project:write` (resolve issues)

**Create Token**:
1. Go to Sentry Settings → API → Auth Tokens
2. Create new token with `project:read` and `project:write`
3. Copy to `SENTRY_AUTH_TOKEN` environment variable

**Configuration**:
```json
{
  "sentry": {
    "env": {
      "SENTRY_AUTH_TOKEN": "${SENTRY_AUTH_TOKEN}",
      "SENTRY_ORG": "robson-bot",
      "SENTRY_PROJECT": "robson-v2"
    }
  }
}
```

### Filesystem MCP

**Required**:
- No external dependencies

**Configuration**:
```json
{
  "filesystem": {
    "env": {
      "ALLOWED_DIRECTORIES": "/home/psyctl/apps/robson/v2"
    }
  }
}
```

This restricts filesystem access to the v2 directory only (security best practice).

## Usage Examples

### Example 1: Search for Code Pattern

```
You: Search the codebase for all uses of `async_trait` and show me examples

Claude: [Uses GitHub MCP to search]
I found 15 uses of async_trait across the codebase. Here are the main patterns:

1. robson-exec/src/ports.rs - Port definitions
2. robson-connectors/src/binance.rs - Exchange adapter
3. robson-store/src/repositories.rs - Database repositories

[Shows code snippets from search results]
```

### Example 2: Database Schema Inspection

```
You: Show me the schema for the orders table and suggest an index for performance

Claude: [Uses Postgres MCP to query schema]
The `orders` table has columns: id, symbol, side, quantity, price, status, created_at

Based on typical query patterns, I suggest adding this index:

CREATE INDEX idx_orders_symbol_status ON orders(symbol, status);

This will speed up queries that filter by both symbol and status (common in order matching).
```

### Example 3: Debug Production Error

```
You: Show me the most recent panic in robsond and help me fix it

Claude: [Uses Sentry MCP to fetch error]
I found a panic in robsond from 2 hours ago:

Error: "attempt to divide by zero"
File: robson-engine/src/risk.rs:45
Function: calculate_position_size

Stack trace shows the issue is when `stop_distance` is 0.
Let me add validation to prevent this...
```

### Example 4: Create Issue for Bug

```
You: Create a GitHub issue for the division by zero bug we just found

Claude: [Uses GitHub MCP to create issue]
Created issue #123: "Add validation for zero stop_distance in position sizing"

Issue includes:
- Description of the bug
- Stack trace from Sentry
- Proposed fix
- Labels: bug, risk-management
```

## Best Practices

### 1. Start Minimal

Begin with only **filesystem** MCP enabled:

```json
{
  "mcpServers": {
    "filesystem": { /* ... */ }
  },
  "defaultServers": ["filesystem"]
}
```

Add other servers as needed.

### 2. Use Read-Only Database

For Postgres MCP, create a read-only user:

```sql
CREATE USER claude_readonly WITH PASSWORD 'secure_password';
GRANT CONNECT ON DATABASE robson_v2_dev TO claude_readonly;
GRANT USAGE ON SCHEMA public TO claude_readonly;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO claude_readonly;
```

Use this in `ROBSON_DATABASE_URL`.

### 3. Limit GitHub Scope

Create a token with minimal permissions:
- Only `public_repo` for public repos
- Only `repo` if you need private repo access
- Don't grant `admin:org` or `delete_repo`

### 4. Audit MCP Actions

MCP actions are logged in Claude Code. Review logs periodically:

```bash
tail -f ~/.claude-code/mcp.log
```

### 5. Don't Commit Credentials

Ensure `.mcp.json` is in `.gitignore`:

```bash
echo ".mcp.json" >> .gitignore
echo ".env.mcp" >> .gitignore
```

Only commit `.mcp.example.json` (template without credentials).

## Security Considerations

### Risks

| MCP Server | Risk Level | Mitigation |
|------------|-----------|------------|
| Filesystem | Low | Restrict to v2 directory only |
| GitHub | Medium | Use token with minimal scopes, read-only preferred |
| PostgreSQL | **HIGH** | **ONLY development databases, read-only user** |
| Sentry | Low | Read-only token, no delete permissions |

### Security Checklist

- [ ] Never use production database with write access
- [ ] Never commit `.mcp.json` with real credentials
- [ ] Use read-only tokens where possible
- [ ] Limit filesystem access to project directory
- [ ] Review MCP logs periodically
- [ ] Rotate tokens regularly (every 90 days)
- [ ] Revoke tokens immediately if compromised

## Troubleshooting

### MCP Server Not Starting

**Problem**: Claude Code shows "MCP server failed to start"

**Solution**:
1. Check environment variables are set: `echo $GITHUB_TOKEN`
2. Verify NPX is installed: `npx --version`
3. Test server manually: `npx @modelcontextprotocol/server-github`
4. Check logs: `tail -f ~/.claude-code/mcp.log`

### Database Connection Failed

**Problem**: PostgreSQL MCP can't connect

**Solution**:
1. Verify database is running: `pg_isready`
2. Check connection string format: `postgresql://user:pass@host:port/db`
3. Test connection: `psql $ROBSON_DATABASE_URL -c "SELECT 1"`
4. Check firewall/network settings

### GitHub Rate Limit

**Problem**: "GitHub API rate limit exceeded"

**Solution**:
1. Use authenticated token (higher rate limit)
2. Wait for rate limit reset (shown in error message)
3. Cache results when possible
4. Reduce frequency of searches

### Sentry Access Denied

**Problem**: "Unauthorized: Invalid token"

**Solution**:
1. Verify token has correct permissions: `project:read`, `project:write`
2. Check organization/project names match
3. Regenerate token if expired
4. Verify token in Sentry UI

## Optional: Pre-Warm MCP Servers

To reduce latency on first use, pre-install and cache MCP servers:

```bash
# Install all MCP servers
npm install -g \
  @modelcontextprotocol/server-github \
  @modelcontextprotocol/server-postgres \
  @modelcontextprotocol/server-sentry \
  @modelcontextprotocol/server-filesystem

# Verify installations
which mcp-server-github
which mcp-server-postgres
```

Update `.mcp.json` to use global installations instead of `npx`.

## Future Enhancements

Potential MCP servers to add later:

- **Slack**: Post notifications for builds/deployments
- **AWS**: Query CloudWatch logs, inspect ECS tasks
- **Datadog**: Monitor application metrics
- **Linear**: Create/update tasks from development workflow

## Further Reading

- [MCP Specification](https://modelcontextprotocol.io/)
- [Claude Code MCP Guide](https://docs.anthropic.com/claude-code/mcp)
- [MCP Server List](https://github.com/modelcontextprotocol/servers)

---

**Version**: 1.0
**Last Updated**: 2026-01-15

**Remember**: MCP is optional but powerful. Start small, add integrations as needed, and always prioritize security.
