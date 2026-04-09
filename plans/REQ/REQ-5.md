# REQ-5: Synapse Support for Hub-Level Security Declarations

**blocked_by:** [REQ-4]
**unlocks:** []
**touches:** plexus-protocol, synapse

## Status: Planned

## Goal

Synapse should understand hub-level security declarations from REQ-4 — showing auth requirements in help output, proactively erroring when credentials are missing, and falling back gracefully when a `-32001` comes back. It already has token forwarding machinery; this connects it to the schema.

## Current State

Synapse already does the right thing at the wire level:

- `--token <jwt>` or `--token-file <path>` → sent as `Cookie: access_token=<jwt>` on WS upgrade
- Token file fallback at `~/.plexus/tokens/<backend>`
- `SYNAPSE_TOKEN` env var: **not implemented** (only the CLI flag and file exist)

What it does not do:

- **Schema**: `PluginSchema` has no `security` field. Hub-level `validate`/`extract` declarations (REQ-4) have nowhere to go in the wire protocol.
- **IR**: `irPlugins` maps `namespace → [method_names]`. No plugin-level metadata slot.
- **-32001 handling**: Authentication errors are rendered as generic `"Error: ..."` — no special message, no hint to use `--token`.
- **Help**: `renderSchema` does not show whether a hub requires authentication.
- **Proactive check**: Synapse doesn't know before calling a method that it will fail with -32001.

## Changes Required

### 1. `plexus-protocol` — Add `PluginSecurity` to `PluginSchema`

The Haskell protocol types are the source of truth for the wire schema. Add a `security` field:

```haskell
-- Plexus/Schema/Recursive.hs

data SecurityValidator = SecurityValidator
  { svName   :: Text          -- e.g. "require_authenticated", "require_cors"
  , svParams :: Maybe Value   -- optional JSON params, e.g. ["https://app.example.com"]
  } deriving (Show, Eq, Generic)

instance FromJSON SecurityValidator
instance ToJSON SecurityValidator

data SecurityExtractor = SecurityExtractor
  { seName     :: Text    -- declared name in hub, e.g. "peer_addr"
  , seTypeName :: Text    -- Rust type name, e.g. "Option<SocketAddr>"
  } deriving (Show, Eq, Generic)

instance FromJSON SecurityExtractor
instance ToJSON SecurityExtractor

data PluginSecurity = PluginSecurity
  { psSecValidators :: [SecurityValidator]
  , psSecExtractors :: [SecurityExtractor]
  } deriving (Show, Eq, Generic)

instance FromJSON PluginSecurity
instance ToJSON PluginSecurity

-- In PluginSchema, add:
data PluginSchema = PluginSchema
  { ...existing fields...
  , psSecurity :: Maybe PluginSecurity   -- Nothing = no security declarations
  }
```

On the Rust side, `plexus-core` serializes `PluginSchema` when responding to schema requests — add the `security` field to the Rust `PluginSchema` struct and populate it from the hub's `validate`/`extract` macro declarations.

### 2. `synapse/src/Synapse/IR/Types.hs` — Add `PluginMeta`

```haskell
data PluginMeta = PluginMeta
  { pmDescription :: Text
  , pmVersion     :: Text
  , pmSecurity    :: Maybe PluginSecurity
  } deriving (Show, Eq, Generic)

data IR = IR
  { ...existing fields...
  , irPluginMeta :: Map Text PluginMeta   -- namespace → metadata
  }

emptyIR :: IR
emptyIR = IR { ...existing..., irPluginMeta = Map.empty }
```

### 3. `synapse/src/Synapse/IR/Builder.hs` — Populate in `irAlgebra`

In the `PluginF schema path childIRs` branch, capture security metadata:

```haskell
irAlgebra (PluginF schema path childIRs) = do
  let namespace = T.intercalate "." path
  let meta = PluginMeta
        { pmDescription = psDescription schema
        , pmVersion     = psVersion schema
        , pmSecurity    = psSecurity schema
        }
  pure $ IR { ...existing...
    , irPluginMeta = Map.insert namespace meta (irPluginMeta childIR)
    }
```

### 4. `synapse/src/Synapse/Algebra/Render.hs` — Show auth requirement in help

When rendering a plugin's help (`renderSchema`), prepend a security notice if validators are present:

```haskell
renderPluginSecurity :: PluginSecurity -> Text
renderPluginSecurity sec =
  let validators = map svName (psSecValidators sec)
      notice = if "require_authenticated" `elem` validators
               then "⚠  Authentication required (use --token <jwt> or SYNAPSE_TOKEN)\n\n"
               else ""
  in notice

renderSchema :: PluginSchema -> Text
renderSchema schema =
  let secNotice = maybe "" renderPluginSecurity (psSecurity schema)
  in secNotice <> ...existing render logic...
```

When synapse shows `synapse backend clients` and the clients hub declares `require_authenticated`, the help output shows the auth notice before the method list.

### 5. `synapse/app/Main.hs` — `SYNAPSE_TOKEN` env var + -32001 hint

**Add `SYNAPSE_TOKEN`** to `resolveToken`:

```haskell
resolveToken :: SynapseOpts -> Text -> IO (Maybe Text)
resolveToken opts backend =
  case soToken opts of
    Just tok -> pure (Just tok)
    Nothing -> do
      mEnvTok <- fmap (fmap T.pack) (lookupEnv "SYNAPSE_TOKEN")
      case mEnvTok of
        Just tok -> pure (Just tok)
        Nothing -> ...existing file lookup...
```

Priority: `--token` → `--token-file` → `SYNAPSE_TOKEN` → `~/.plexus/tokens/<backend>`

**Intercept -32001 in error rendering**:

```haskell
-- In printResult or the StreamError handler:
renderError :: SynapseError -> Text
renderError (RpcError (-32001) msg _) =
  "Authentication required: " <> msg <>
  "\nUse --token <jwt>, --token-file <path>, or set SYNAPSE_TOKEN."
renderError err = ...existing...
```

### 6. `synapse/src/Synapse/Algebra/Navigate.hs` — Proactive auth check (optional)

After fetching a schema during navigation, if `psSecurity` declares `require_authenticated` and `seToken` is `Nothing`, emit a warning before proceeding:

```haskell
ensureCredentials :: PluginSchema -> SynapseM ()
ensureCredentials schema =
  case psSecurity schema of
    Nothing -> pure ()
    Just sec | "require_authenticated" `elem` map svName (psSecValidators sec) -> do
      mTok <- asks seToken
      when (isNothing mTok) $
        liftIO $ TIO.hPutStrLn stderr
          "Warning: this hub requires authentication but no token is configured."
    _ -> pure ()
```

This is a warning, not a hard failure — the actual rejection comes from the server.

## Non-Changes (Already Correct)

- **Origin validation**: Synapse does not send an `Origin` header (it's not a browser). The `require_cors` validator's `None` arm allows non-browser clients through. No synapse change needed for CORS.
- **Extractor parameters** (`#[from_hub]`): Hub extractors (`peer_addr`, `origin`) that a method opts into via `#[from_hub]` are stripped from the RPC schema by the macro — same as `#[from_auth]`. Synapse never sees them as parameters. No change needed.
- **Cookie format**: The existing `cookieHeader` sends `access_token=<jwt>` which matches what `CombinedAuthMiddleware` looks for.

## File Summary

| File | Repo | Change |
|------|------|--------|
| `src/Plexus/Schema/Recursive.hs` | plexus-protocol | Add `PluginSecurity`, `SecurityValidator`, `SecurityExtractor`; add `psSecurity` to `PluginSchema` |
| `src/plexus/plexus.rs` (or schema serialization) | plexus-core | Serialize `security` field in `PluginSchema` JSON from hub macro declarations |
| `src/Synapse/IR/Types.hs` | synapse | Add `PluginMeta`, `irPluginMeta` to `IR` |
| `src/Synapse/IR/Builder.hs` | synapse | Populate `irPluginMeta` in `PluginF` branch |
| `src/Synapse/Algebra/Render.hs` | synapse | Show auth notice in hub help output |
| `app/Main.hs` | synapse | Add `SYNAPSE_TOKEN` env var; intercept -32001 with auth hint |
| `src/Synapse/Algebra/Navigate.hs` | synapse | `ensureCredentials` warning (optional) |

## Acceptance Criteria

- [ ] `synapse backend clients` (hub with `require_authenticated`) shows auth notice in help
- [ ] `synapse backend clients list` with no token returns "Authentication required. Use --token..."
- [ ] `SYNAPSE_TOKEN=<jwt> synapse backend clients list` authenticates successfully
- [ ] `synapse --token <jwt> backend clients list` continues to work (existing behavior)
- [ ] `synapse backend forms list` (hub with no security declaration) unaffected
- [ ] `require_cors` validator does not block synapse (no Origin header = allowed through)
- [ ] Hub extractor parameters (`#[from_hub]`) do not appear in synapse's method help (stripped by macro)

## Tests

### Haskell unit tests — `synapse/test/` (HSpec or Tasty)

**`renderPluginSecurity`:**
```haskell
describe "renderPluginSecurity" $ do
  it "includes auth notice when require_authenticated is a validator" $ do
    let sec = PluginSecurity
          { psSecValidators = [SecurityValidator "require_authenticated" Nothing]
          , psSecExtractors = []
          }
    renderPluginSecurity sec `shouldContain` "Authentication required"
    renderPluginSecurity sec `shouldContain` "--token"
    renderPluginSecurity sec `shouldContain` "SYNAPSE_TOKEN"

  it "returns empty string when no validators" $ do
    let sec = PluginSecurity { psSecValidators = [], psSecExtractors = [] }
    renderPluginSecurity sec `shouldBe` ""

  it "does not show auth notice for non-auth validators" $ do
    let sec = PluginSecurity
          { psSecValidators = [SecurityValidator "require_cors" (Just ["https://app.example.com"])]
          , psSecExtractors = []
          }
    renderPluginSecurity sec `shouldNotContain` "Authentication required"
```

**`resolveToken` — SYNAPSE_TOKEN env var:**
```haskell
describe "resolveToken" $ do
  it "returns SYNAPSE_TOKEN when set and no --token flag" $
    withEnv [("SYNAPSE_TOKEN", "env-jwt")] $ do
      result <- resolveToken defaultOpts "mybackend"
      result `shouldBe` Just "env-jwt"

  it "--token flag takes priority over SYNAPSE_TOKEN" $
    withEnv [("SYNAPSE_TOKEN", "env-jwt")] $ do
      result <- resolveToken (defaultOpts { soToken = Just "flag-jwt" }) "mybackend"
      result `shouldBe` Just "flag-jwt"

  it "falls through to file when SYNAPSE_TOKEN absent" $
    withEnv [] $ do
      -- No env var, no flag, no file → Nothing
      result <- resolveToken defaultOpts "nonexistent-backend"
      result `shouldBe` Nothing
```

**`renderError` — -32001 hint:**
```haskell
describe "renderError" $ do
  it "includes --token hint on -32001" $ do
    let err = RpcError (-32001) "Authentication required: no token" Nothing
    renderError err `shouldContain` "--token"
    renderError err `shouldContain` "SYNAPSE_TOKEN"

  it "renders other errors without token hint" $ do
    let err = RpcError (-32000) "Execution error" Nothing
    renderError err `shouldNotContain` "SYNAPSE_TOKEN"

  it "includes original message in -32001 output" $ do
    let err = RpcError (-32001) "This method requires authentication" Nothing
    renderError err `shouldContain` "This method requires authentication"
```

**`PluginSchema` JSON roundtrip — security field:**
```haskell
describe "PluginSchema JSON" $ do
  it "decodes psSecurity when present" $ do
    let json = [aesonQQ| {
          "namespace": "clients", "version": "1.0.0", "description": "...",
          "methods": [], "hash": "abc",
          "security": {
            "validators": [{"name": "require_authenticated"}],
            "extractors": []
          }
        } |]
    let schema = decode json :: Maybe PluginSchema
    (psSecurity =<< schema) `shouldSatisfy` isJust

  it "psSecurity is Nothing when field absent" $ do
    let json = [aesonQQ| {"namespace":"x","version":"1","description":"","methods":[],"hash":"a"} |]
    let schema = fromJust $ decode json :: PluginSchema
    psSecurity schema `shouldBe` Nothing
```

### CLI integration tests — `synapse/test/cli-test/` (requires running backend)

These require a backend server with a `clients` hub (REQ-4 `require_authenticated`) and a `forms` hub (no security).

```
-- Auth notice in help:
synapse backend clients
  stdout must contain "Authentication required"
  stdout must contain "--token" or "SYNAPSE_TOKEN"

-- Auth notice absent for unsecured hub:
synapse backend forms
  stdout must NOT contain "Authentication required"

-- -32001 error message:
synapse backend clients list   (no token)
  stderr must contain "Authentication required"
  stderr must contain "--token"
  exit code must be non-zero

-- SYNAPSE_TOKEN env var:
SYNAPSE_TOKEN=<valid-jwt> synapse backend clients list
  exit code == 0
  stdout contains client data

-- --token flag (existing behavior):
synapse --token <valid-jwt> backend clients list
  exit code == 0

-- CORS: synapse has no Origin header → require_cors passes:
synapse --token <valid-jwt> backend clients list
  exit code == 0   (even if server has require_cors validator)

-- Hub extractor params stripped from method help:
synapse --token <valid-jwt> backend clients list --help
  stdout must NOT contain "peer_addr"
  stdout must NOT contain "origin" as a parameter name
```
