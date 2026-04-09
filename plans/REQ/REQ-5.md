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
- **Help**: `renderSchema` does not show whether a hub requires authentication or what cookies/headers it needs.
- **Proactive check**: Synapse doesn't know before calling a method that it will fail with -32001.
- **Generic flags**: No `--cookie`, `--header`, or `--query` flags for satisfying arbitrary request contracts.

## Changes Required

### 1. `plexus-protocol` — Add `PluginSecurity` with `ContractEntry` to `PluginSchema`

The Haskell protocol types are the source of truth for the wire schema. The `security` field exposes a **request contract** — a client-facing description of what HTTP-level data the hub needs on WS upgrade. This is NOT Rust type information; it's transport-level routing information.

```haskell
-- Plexus/Schema/Recursive.hs

data SecurityValidator = SecurityValidator
  { svName   :: Text          -- e.g. "require_authenticated", "require_cors"
  , svParams :: Maybe Value   -- optional JSON params, e.g. ["https://app.example.com"]
  } deriving (Show, Eq, Generic)

instance FromJSON SecurityValidator
instance ToJSON SecurityValidator

-- Where the client must put the data on WS upgrade.
-- Derived = computed from network state server-side; client cannot supply it.
data ContractSource
  = ContractCookie      -- read from Cookie header
  | ContractHeader      -- read from an HTTP header
  | ContractQueryParam  -- read from URI query string
  | ContractDerived     -- computed server-side (peer_addr, etc.)
  deriving (Show, Eq, Generic)

instance FromJSON ContractSource
instance ToJSON ContractSource

-- One entry per HTTP-level input the hub's extractors need from the client.
data ContractEntry = ContractEntry
  { ceSource      :: ContractSource
  , ceKey         :: Maybe Text      -- header/cookie/param name; Nothing for Derived
  , ceRequired    :: Bool
  , ceDescription :: Text
  } deriving (Show, Eq, Generic)

instance FromJSON ContractEntry
instance ToJSON ContractEntry

data PluginSecurity = PluginSecurity
  { psValidators      :: [SecurityValidator]
  , psRequestContract :: [ContractEntry]    -- what the client must send on WS upgrade
  } deriving (Show, Eq, Generic)

instance FromJSON PluginSecurity
instance ToJSON PluginSecurity

-- In PluginSchema, add:
data PluginSchema = PluginSchema
  { ...existing fields...
  , psSecurity :: Maybe PluginSecurity   -- Nothing = no security declarations
  }
```

`SecurityExtractor` (the old `{ seName, seTypeName }` form with Rust type strings) is removed entirely. Rust type names are meaningless to Haskell clients and don't tell clients where to put data.

On the Rust side, `plexus-core` serializes `PluginSchema` when responding to schema requests — add the `security` field to the Rust `PluginSchema` struct and populate it from the hub's `validate`/`extract` macro declarations (see REQ-4 Codegen section for how the macro infers `ContractEntry` per extractor).

Wire format example:

```json
{
  "security": {
    "validators": [
      {"name": "require_authenticated"},
      {"name": "require_cors", "params": ["https://app.formveritas.com"]}
    ],
    "request_contract": [
      {"source": "Cookie", "key": "access_token", "required": true,  "description": "JWT auth token"},
      {"source": "Header", "key": "origin",       "required": false, "description": "Origin header for CORS"},
      {"source": "Derived", "key": null,           "required": false, "description": "server-derived: extract_peer_addr"}
    ]
  }
}
```

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

### Request Contract Philosophy

The `request_contract` field is client-facing, not type-facing. It answers the question "what do I need to send?" not "what Rust type does the server produce?".

- `ContractEntry { source: Cookie, key: "access_token", required: true }` → synapse sends `Cookie: access_token=<value>` on WS upgrade
- `ContractEntry { source: Header, key: "origin", required: false }` → synapse may send `Origin: <value>` if configured
- `ContractEntry { source: Derived }` → synapse cannot supply this; it's computed server-side (peer address, etc.)

ContractEntries are derived from the extractor stdlib at macro-expansion time (see REQ-4 for the inference rules). The server processes the raw request internally — the contract is only for clients to understand what to send.

Validators like `require_authenticated` don't directly produce ContractEntries, but `require_authenticated` implies a Cookie entry for `access_token` because that's what the auth middleware reads. This is the one case where a validator drives a contract entry. Other validators (`require_cors`) are purely server-side gates — the origin header is a browser concern, not a synapse concern.

### 4. `synapse/src/Synapse/Algebra/Render.hs` — Show request contract in help

When rendering a plugin's help (`renderSchema`), show the request contract if present:

```haskell
renderContractSource :: ContractSource -> Text
renderContractSource ContractCookie     = "Cookie"
renderContractSource ContractHeader     = "Header"
renderContractSource ContractQueryParam = "QueryParam"
renderContractSource ContractDerived    = "Server-derived"

renderContractEntry :: ContractEntry -> Text
renderContractEntry e =
  "  " <> renderContractSource (ceSource e)
       <> maybe "" (" " <>) (ceKey e)
       <> (if ceRequired e then " (required)" else " (optional)")
       <> ": " <> ceDescription e

renderRequestContract :: [ContractEntry] -> Text
renderRequestContract [] = ""
renderRequestContract entries =
  "Request requirements:\n" <> T.unlines (map renderContractEntry entries)

renderPluginSecurity :: PluginSecurity -> Text
renderPluginSecurity sec =
  let validators = map svName (psValidators sec)
      authNotice = if "require_authenticated" `elem` validators
                   then "Authentication required (use --token <jwt> or SYNAPSE_TOKEN)\n\n"
                   else ""
      contractBlock = renderRequestContract (psRequestContract sec)
  in authNotice <> contractBlock

renderSchema :: PluginSchema -> Text
renderSchema schema =
  let secBlock = maybe "" renderPluginSecurity (psSecurity schema)
  in secBlock <> ...existing render logic...
```

Example output for the `clients` hub:

```
Authentication required (use --token <jwt> or SYNAPSE_TOKEN)

Request requirements:
  Cookie access_token (required): JWT auth token
  Header origin (optional): Origin header for CORS
  Server-derived (optional): server-derived: extract_peer_addr

Methods:
  list    List clients
  get     Get a client by ID
  ...
```

### 5. `synapse/app/Main.hs` — Generic contract flags + `SYNAPSE_TOKEN` + -32001 hint

**Add `--cookie`/`--header`/`--query` flags** for satisfying arbitrary request contracts:

```haskell
data SynapseOpts = SynapseOpts
  { ...existing...
  , soCookies :: [(Text, Text)]    -- --cookie key=value (parsed as "key=value")
  , soHeaders :: [(Text, Text)]    -- --header key=value
  , soQuery   :: [(Text, Text)]    -- --query key=value (appended to WS upgrade URI)
  }

-- Parsing: --cookie access_token=<jwt> → ("access_token", "<jwt>")
-- Multiple --cookie flags are accumulated as a list.

-- Env var support (checked at startup, merged before building the request):
-- SYNAPSE_COOKIE_<UPPERCASE_KEY> → added to soCookies automatically
-- SYNAPSE_HEADER_<UPPERCASE_KEY> → added to soHeaders automatically

-- In buildRequest / cookieHeader:
-- Merge soCookies + existing --token (which becomes access_token cookie) into Cookie header
-- Add soHeaders as extra WS upgrade request headers
-- Add soQuery params to the WS upgrade URI query string
```

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

Priority: `--token` → `--token-file` → `SYNAPSE_TOKEN` → `SYNAPSE_COOKIE_ACCESS_TOKEN` → `~/.plexus/tokens/<backend>`

**Intercept -32001 in error rendering**:

```haskell
-- In printResult or the StreamError handler:
renderError :: SynapseError -> Text
renderError (RpcError (-32001) msg _) =
  "Authentication required: " <> msg <>
  "\nUse --token <jwt>, --token-file <path>, set SYNAPSE_TOKEN, or --cookie access_token=<jwt>."
renderError err = ...existing...
```

### 6. `synapse/src/Synapse/Algebra/Navigate.hs` — Proactive contract check

After fetching a schema during navigation, check the request contract against configured data:

```haskell
ensureContractSatisfied :: PluginSchema -> SynapseM ()
ensureContractSatisfied schema =
  case psSecurity schema of
    Nothing -> pure ()
    Just sec -> do
      mTok  <- asks seToken
      cookies <- asks seCookies
      headers <- asks seHeaders
      let contract = psRequestContract sec
      forM_ contract $ \entry ->
        when (ceRequired entry && ceSource entry /= ContractDerived) $ do
          let satisfied = case ceSource entry of
                ContractCookie ->
                  -- access_token covered by --token; other cookies from seCookies
                  maybe False (\k -> k == "access_token" && isJust mTok
                                  || any ((== k) . fst) cookies)
                              (ceKey entry)
                ContractHeader ->
                  maybe False (\k -> any ((== k) . fst) headers) (ceKey entry)
                ContractQueryParam ->
                  True  -- query params always optional in practice; skip for now
                ContractDerived -> True
          unless satisfied $
            liftIO $ TIO.hPutStrLn stderr $
              "Warning: hub requires " <> renderContractSource (ceSource entry) <>
              maybe "" (" " <>) (ceKey entry) <>
              " but none is configured."
```

This is a warning, not a hard failure — the actual rejection comes from the server. The check runs before invoking a method, not when showing help.

## Non-Changes (Already Correct)

- **Origin validation**: Synapse does not send an `Origin` header (it's not a browser). The `require_cors` validator's `None` arm allows non-browser clients through. No synapse change needed for CORS.
- **Extractor parameters** (`#[from_hub]`): Hub extractors (`peer_addr`, `origin`) that a method opts into via `#[from_hub]` are stripped from the RPC schema by the macro — same as `#[from_auth]`. Synapse never sees them as parameters. No change needed.
- **Cookie format**: The existing `cookieHeader` sends `access_token=<jwt>` which matches what `CombinedAuthMiddleware` looks for.

## File Summary

| File | Repo | Change |
|------|------|--------|
| `src/Plexus/Schema/Recursive.hs` | plexus-protocol | Add `PluginSecurity`, `SecurityValidator`, `ContractEntry`, `ContractSource`; add `psSecurity` to `PluginSchema`; remove `SecurityExtractor` |
| `src/plexus/plexus.rs` (or schema serialization) | plexus-core | Serialize `security` field in `PluginSchema` JSON from hub macro declarations; use `request_contract` array of `ContractEntry`, not Rust type strings |
| `src/Synapse/IR/Types.hs` | synapse | Add `PluginMeta`, `irPluginMeta` to `IR` |
| `src/Synapse/IR/Builder.hs` | synapse | Populate `irPluginMeta` in `PluginF` branch |
| `src/Synapse/Algebra/Render.hs` | synapse | `renderRequestContract`, `renderContractEntry`; show contract in hub help output |
| `app/Main.hs` | synapse | Add `--cookie`/`--header`/`--query` flags; `SYNAPSE_TOKEN` env var; env var scanning for `SYNAPSE_COOKIE_*`/`SYNAPSE_HEADER_*`; intercept -32001 with auth hint |
| `src/Synapse/Algebra/Navigate.hs` | synapse | `ensureContractSatisfied` proactive warning |

## Acceptance Criteria

- [ ] `synapse backend clients` (hub with `require_authenticated`) shows "Authentication required" and "Request requirements: Cookie access_token (required)" in help
- [ ] `synapse backend clients list` with no token returns "Authentication required. Use --token..."
- [ ] `SYNAPSE_TOKEN=<jwt> synapse backend clients list` authenticates successfully
- [ ] `synapse --token <jwt> backend clients list` continues to work (existing behavior)
- [ ] `synapse --cookie access_token=<jwt> backend clients list` authenticates successfully
- [ ] `SYNAPSE_COOKIE_ACCESS_TOKEN=<jwt> synapse backend clients list` authenticates successfully
- [ ] `synapse backend forms list` (hub with no security declaration) unaffected
- [ ] `require_cors` validator does not block synapse (no Origin header = allowed through)
- [ ] Hub extractor parameters (`#[from_hub]`) do not appear in synapse's method help (stripped by macro)
- [ ] Wire schema uses `request_contract` array with `ContractEntry` objects — no Rust type strings in schema output

## Tests

### Haskell unit tests — `synapse/test/` (HSpec or Tasty)

**`renderRequestContract`:**
```haskell
describe "renderRequestContract" $ do
  it "shows Cookie access_token (required) for auth token entry" $ do
    let entry = ContractEntry
          { ceSource = ContractCookie, ceKey = Just "access_token"
          , ceRequired = True, ceDescription = "JWT auth token" }
    renderRequestContract [entry] `shouldContain` "Cookie access_token (required)"
    renderRequestContract [entry] `shouldContain` "JWT auth token"

  it "returns empty string for empty contract" $
    renderRequestContract [] `shouldBe` ""

  it "shows Server-derived for Derived source" $ do
    let entry = ContractEntry
          { ceSource = ContractDerived, ceKey = Nothing
          , ceRequired = False, ceDescription = "peer address" }
    renderRequestContract [entry] `shouldContain` "Server-derived"
```

**`renderPluginSecurity`:**
```haskell
describe "renderPluginSecurity" $ do
  it "includes auth notice when require_authenticated is a validator" $ do
    let sec = PluginSecurity
          { psValidators      = [SecurityValidator "require_authenticated" Nothing]
          , psRequestContract = [ContractEntry ContractCookie (Just "access_token") True "JWT auth token"]
          }
    renderPluginSecurity sec `shouldContain` "Authentication required"
    renderPluginSecurity sec `shouldContain` "--token"
    renderPluginSecurity sec `shouldContain` "SYNAPSE_TOKEN"
    renderPluginSecurity sec `shouldContain` "Cookie access_token (required)"

  it "returns empty string when no validators and empty contract" $ do
    let sec = PluginSecurity { psValidators = [], psRequestContract = [] }
    renderPluginSecurity sec `shouldBe` ""

  it "does not show auth notice for non-auth validators" $ do
    let sec = PluginSecurity
          { psValidators      = [SecurityValidator "require_cors" (Just ["https://app.example.com"])]
          , psRequestContract = []
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

**`PluginSchema` JSON roundtrip — `request_contract` field:**
```haskell
describe "PluginSchema JSON" $ do
  it "decodes psSecurity with request_contract when present" $ do
    let json = [aesonQQ| {
          "namespace": "clients", "version": "1.0.0", "description": "...",
          "methods": [], "hash": "abc",
          "security": {
            "validators": [{"name": "require_authenticated"}],
            "request_contract": [
              {"source": "Cookie", "key": "access_token", "required": true, "description": "JWT auth token"}
            ]
          }
        } |]
    let schema = decode json :: Maybe PluginSchema
    let contract = psRequestContract =<< (psSecurity =<< schema)
    contract `shouldSatisfy` (not . null)
    fmap ceSource (listToMaybe =<< contract) `shouldBe` Just ContractCookie

  it "psSecurity is Nothing when field absent" $ do
    let json = [aesonQQ| {"namespace":"x","version":"1","description":"","methods":[],"hash":"a"} |]
    let schema = fromJust $ decode json :: PluginSchema
    psSecurity schema `shouldBe` Nothing

  it "does not decode SecurityExtractor type_name field (removed)" $ do
    -- Old format with seTypeName should not be accepted
    let json = [aesonQQ| {
          "security": {
            "validators": [],
            "extractors": [{"name": "peer_addr", "type_name": "Option<SocketAddr>"}]
          }
        } |]
    -- extractors key is unknown; request_contract should be empty
    -- (exact behavior depends on aeson strictness settings)
    pure ()  -- presence of old key must not cause a parse error
```

### CLI integration tests — `synapse/test/cli-test/` (requires running backend)

These require a backend server with a `clients` hub (REQ-4 `require_authenticated`) and a `forms` hub (no security).

```
-- Auth notice and contract in help:
synapse backend clients
  stdout must contain "Authentication required"
  stdout must contain "Cookie access_token (required)"
  stdout must contain "--token" or "SYNAPSE_TOKEN"

-- Auth notice absent for unsecured hub:
synapse backend forms
  stdout must NOT contain "Authentication required"
  stdout must NOT contain "Request requirements"

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

-- --cookie flag:
synapse --cookie access_token=<valid-jwt> backend clients list
  exit code == 0

-- SYNAPSE_COOKIE_ACCESS_TOKEN env var:
SYNAPSE_COOKIE_ACCESS_TOKEN=<valid-jwt> synapse backend clients list
  exit code == 0

-- CORS: synapse has no Origin header → require_cors passes:
synapse --token <valid-jwt> backend clients list
  exit code == 0   (even if server has require_cors validator)

-- Hub extractor params stripped from method help:
synapse --token <valid-jwt> backend clients list --help
  stdout must NOT contain "peer_addr"
  stdout must NOT contain "origin" as a parameter name
```

## Open Design Questions

**Q1: Should `ContractEntry` include JSON Schema type information for the expected value?**

No for v1. `ContractEntry` is about transport-level routing (where to put data), not type validation. JSON Schema type info would be needed for code generation tools (OpenAPI generators, typed client SDKs) that need to know the shape of the value. Synapse only needs to know where to put it. Add type info later if a code gen use case materializes.

**Q2: Should `require_authenticated` generate a `ContractEntry`?**

Yes — since `require_authenticated` checks `ctx.auth`, and auth comes from the `access_token` cookie (via `CombinedAuthMiddleware`), the macro should infer `ContractEntry { source: Cookie, key: "access_token", required: true }` for hubs with this validator in their `validate` list. This is the one case where a validator drives a contract entry, because the cookie is the specific transport mechanism auth depends on. Synapse's existing `--token` flag already sends this cookie; the contract entry makes the requirement explicit in help output and the proactive check.

**Q3: What about user-defined extractor functions not in the stdlib?**

Default to `ContractDerived`. The macro cannot infer transport semantics for arbitrary user functions. The contract entry will appear in the schema as `{ source: "Derived", key: null, required: false }` with a description like `"server-derived: my_extractor_fn"`. Synapse can show this in help so the user knows something is happening server-side, but cannot help supply the value. If the user wants synapse to supply data for a custom extractor, they should implement it as a cookie/header extractor (which the macro recognizes) rather than a completely opaque function.
