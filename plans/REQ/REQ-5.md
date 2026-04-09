# REQ-5: Synapse Support for Hub Request Schemas

**blocked_by:** [REQ-4]
**unlocks:** []
**touches:** plexus-protocol, synapse

## Status: Planned

## Goal

Synapse reads the `psRequest` JSON Schema blob from `PluginSchema`, shows auth requirements and request fields in help output, proactively warns when required fields aren't configured, and provides `--cookie`/`--header`/`--query` flags and environment variable shorthands to satisfy those requirements. The wire format change is minimal: one new field (`request: Maybe Value`) on `PluginSchema`.

## What Changes in the Wire Format

### `plexus-protocol` — Add `psRequest` to `PluginSchema`

The Haskell protocol types are the source of truth for the wire schema. The only change is adding one optional field:

```haskell
-- Plexus/Schema/Recursive.hs

data PluginSchema = PluginSchema
  { psNamespace   :: Text
  , psVersion     :: Text
  , psDescription :: Text
  , psMethods     :: [MethodSchema]
  , psHash        :: Text
  -- NEW:
  , psRequest     :: Maybe Value
    -- JSON Schema of the hub's PlexusRequest struct, or Nothing if no request shape.
    -- Fields annotated with x-plexus-source describe where the client should put data.
  } deriving (Show, Eq, Generic)

instance FromJSON PluginSchema where
  parseJSON = withObject "PluginSchema" $ \o -> PluginSchema
    <$> o .: "namespace"
    <*> o .: "version"
    <*> o .: "description"
    <*> o .: "methods"
    <*> o .: "hash"
    <*> o .:? "request"   -- optional: old servers without REQ-4 send Nothing

instance ToJSON PluginSchema
```

That's it. `ContractEntry`, `ContractSource`, `SecurityValidator`, `SecurityExtractor`, `PluginSecurity` — none of these are added to the Haskell types. The wire format for request requirements IS the JSON Schema of the Rust struct, passed as an opaque `Value`. Synapse reads it directly.

Removed from plexus-protocol (if they existed as draft types): `SecurityExtractor` (the old `{ seName, seTypeName }` form with Rust type strings). Rust type names in the wire schema were never correct.

On the Rust side (`plexus-core`): add `request: Option<Value>` to the `PluginSchema` struct and populate it from `schemars::schema_for!(RequestType)` in `plugin_schema()`.

## Synapse Changes

### 1. `synapse/src/Synapse/IR/Types.hs` — Add `PluginMeta`

```haskell
data PluginMeta = PluginMeta
  { pmDescription :: Text
  , pmVersion     :: Text
  , pmRequest     :: Maybe Value   -- JSON Schema of hub's request struct; Nothing = no shape
  } deriving (Show, Eq, Generic)

data IR = IR
  { irTypes      :: Map TypeHash IRType
  , irMethods    :: Map MethodPath IRMethod
  , irPluginMeta :: Map Text PluginMeta   -- namespace → metadata (NEW)
  }

emptyIR :: IR
emptyIR = IR { irTypes = Map.empty, irMethods = Map.empty, irPluginMeta = Map.empty }
```

### 2. `synapse/src/Synapse/IR/Builder.hs` — Populate in `irAlgebra`

In the `PluginF schema path childIRs` branch:

```haskell
irAlgebra (PluginF schema path childIRs) = do
  let namespace = T.intercalate "." path
      meta = PluginMeta
        { pmDescription = psDescription schema
        , pmVersion     = psVersion schema
        , pmRequest     = psRequest schema
        }
  pure $ IR
    { irTypes      = mergedTypes
    , irMethods    = mergedMethods
    , irPluginMeta = Map.insert namespace meta (foldMap irPluginMeta childIRs)
    }
```

### 3. `synapse/src/Synapse/Algebra/Render.hs` — Request schema in help

Synapse walks the `psRequest` JSON Schema to build the help output. The `x-plexus-source` extension field on each property tells synapse what it is and where it comes from.

```haskell
-- Walk the request schema's properties and render each field
renderRequestSchema :: Value -> Text
renderRequestSchema schema =
  let props    = fromMaybe Map.empty (schema ^? key "properties" . _Object)
      required = fromMaybe [] (schema ^? key "required" . _Array
                   <&> V.toList <&> mapMaybe (^? _String))
      entries  = Map.toList props
  in if null entries then ""
     else "Request requirements:\n" <> T.unlines (map (renderRequestField required) entries)

renderRequestField :: [Text] -> (Text, Value) -> Text
renderRequestField required (name, propSchema) =
  let source    = propSchema ^? key "x-plexus-source"
      fromVal   = source >>= (^? key "from" . _String)
      keyVal    = source >>= (^? key "key" . _String)
      isReq     = name `elem` required
      isDerived = fromVal == Just "derived"
      desc      = fromMaybe "" (propSchema ^? key "description" . _String)
      label     = case fromVal of
        Just "cookie"  -> "Cookie"
        Just "header"  -> "Header"
        Just "query"   -> "QueryParam"
        Just "derived" -> "Server-derived"
        _              -> "Unknown"
      keyPart   = maybe "" (" " <>) keyVal
      reqPart   = if isReq && not isDerived then " (required)" else " (optional)"
  in "  " <> label <> keyPart <> reqPart
     <> (if T.null desc then "" else ": " <> desc)

renderPluginHelp :: PluginSchema -> IR -> Text
renderPluginHelp schema ir =
  let reqBlock = maybe "" renderRequestSchema (psRequest schema)
      authLine  = case psRequest schema of
        Nothing -> ""
        Just s  ->
          let required = fromMaybe [] (s ^? key "required" . _Array
                            <&> V.toList <&> mapMaybe (^? _String))
              props    = fromMaybe Map.empty (s ^? key "properties" . _Object)
              hasCookieAuth = any (\n ->
                let src = Map.lookup n props >>= (^? key "x-plexus-source")
                in (src >>= (^? key "from" . _String)) == Just "cookie"
                && (src >>= (^? key "key" . _String)) == Just "access_token"
                && n `elem` required) required
          in if hasCookieAuth
             then "Authentication required (use --cookie access_token=<jwt>, --token <jwt>, or SYNAPSE_TOKEN)\n\n"
             else ""
  in authLine <> reqBlock <> "\n" <> ...existing method render...
```

Example output for the `clients` hub:

```
Authentication required (use --cookie access_token=<jwt>, --token <jwt>, or SYNAPSE_TOKEN)

Request requirements:
  Cookie access_token (required): JWT from Keycloak auth flow
  Header origin (optional)
  Server-derived (optional): Caller IP address

Methods:
  list    List clients
  get     Get a client by ID
  ...
```

### 4. `synapse/app/Main.hs` — `--cookie`/`--header`/`--query` flags and env vars

```haskell
data SynapseOpts = SynapseOpts
  { soHost     :: Text
  , soPort     :: Int
  , soToken    :: Maybe Text
  , soTokenFile :: Maybe FilePath
  , soJson     :: Bool
  -- NEW:
  , soCookies  :: [(Text, Text)]    -- --cookie key=value pairs
  , soHeaders  :: [(Text, Text)]    -- --header key=value pairs
  , soQuery    :: [(Text, Text)]    -- --query key=value pairs (appended to WS upgrade URI)
  }

-- Parsing: --cookie access_token=<jwt> → ("access_token", "<jwt>")
-- Multiple --cookie flags accumulate. key=value must contain exactly one '='.

-- Env var scanning at startup:
-- SYNAPSE_COOKIE_<KEY> → added to soCookies as ("key_lowercase", value)
-- SYNAPSE_HEADER_<KEY> → added to soHeaders as ("key_lowercase", value)
-- SYNAPSE_TOKEN        → treated as sugar for SYNAPSE_COOKIE_ACCESS_TOKEN (alias, not deprecated)

-- Token resolution priority:
-- 1. --token flag
-- 2. --token-file
-- 3. SYNAPSE_TOKEN env var
-- 4. SYNAPSE_COOKIE_ACCESS_TOKEN env var
-- 5. ~/.plexus/tokens/<backend>
-- All paths produce a cookie entry: ("access_token", <token>)

-- Building WS upgrade request:
-- Cookie header = merge all soCookies entries (plus token → access_token)
-- Extra headers = soHeaders entries added to WS upgrade request
-- Query string  = soQuery entries appended to the WS upgrade URI
```

### 5. `synapse/src/Synapse/Algebra/Navigate.hs` — Proactive contract check

After schema fetch and before method invocation, warn if required request fields aren't provided:

```haskell
checkRequestSatisfied :: PluginSchema -> SynapseEnv -> IO ()
checkRequestSatisfied schema env =
  case psRequest schema of
    Nothing -> pure ()
    Just reqSchema ->
      let props    = fromMaybe Map.empty (reqSchema ^? key "properties" . _Object)
          required = fromMaybe [] (reqSchema ^? key "required" . _Array
                       <&> V.toList <&> mapMaybe (^? _String))
      in forM_ required $ \fieldName -> do
           let propSchema = Map.lookup fieldName props
               source     = propSchema >>= (^? key "x-plexus-source")
               fromVal    = source >>= (^? key "from" . _String)
               keyVal     = source >>= (^? key "key" . _String)
               isDerived  = fromVal == Just "derived"
           unless isDerived $ do
             let satisfied = case fromVal of
                   Just "cookie" ->
                     let k = fromMaybe fieldName keyVal
                     in k == "access_token" && isJust (seToken env)
                        || any ((== k) . fst) (seCookies env)
                   Just "header" ->
                     let k = fromMaybe fieldName keyVal
                     in any ((== k) . fst) (seHeaders env)
                   Just "query"  -> True  -- not proactively checked in v1
                   _             -> True
             unless satisfied $
               hPutStrLn stderr $
                 "Warning: hub requires "
                 <> maybe (T.unpack fieldName) T.unpack keyVal
                 <> " (" <> maybe "unknown" T.unpack fromVal <> ")"
                 <> " but none is configured."
```

This is a warning, not a hard failure. The server rejects the call if the field is truly absent.

### 6. Error rendering — -32001 hint

```haskell
-- In error rendering:
renderRpcError :: Int -> Text -> Text
renderRpcError (-32001) msg =
  "Authentication required: " <> msg <> "\n"
  <> "Use --token <jwt>, --cookie access_token=<jwt>, or set SYNAPSE_TOKEN."
renderRpcError _ msg = "Error: " <> msg
```

## Non-Changes

- **Origin validation**: Synapse does not send an `Origin` header. The `ValidOrigin` extraction passes for absent origin. No synapse change needed for CORS.
- **`#[from_request]` params on methods**: Stripped by the macro from the RPC schema. Synapse never sees them as method parameters.
- **Cookie format**: The existing `cookieHeader` logic sends `access_token=<jwt>`. Unchanged — it becomes one of the `soCookies` entries.
- **`SYNAPSE_TOKEN`**: Kept as-is, treated as alias for `SYNAPSE_COOKIE_ACCESS_TOKEN`. Not deprecated.

## File Summary

| File | Repo | Change |
|------|------|--------|
| `src/Plexus/Schema/Recursive.hs` | plexus-protocol | Add `psRequest :: Maybe Value` to `PluginSchema`; remove `SecurityExtractor` if it existed as draft |
| `src/plexus/plexus.rs` (schema serialization) | plexus-core | Populate `request` field in `PluginSchema` JSON from `schemars::schema_for!(RequestType)` |
| `src/Synapse/IR/Types.hs` | synapse | Add `PluginMeta`, `irPluginMeta` to `IR` |
| `src/Synapse/IR/Builder.hs` | synapse | Populate `irPluginMeta` in `PluginF` branch |
| `src/Synapse/Algebra/Render.hs` | synapse | `renderRequestSchema`, `renderRequestField`, `renderPluginHelp` with request block |
| `app/Main.hs` | synapse | Add `--cookie`/`--header`/`--query` flags; env var scanning; -32001 error hint |
| `src/Synapse/Algebra/Navigate.hs` | synapse | `checkRequestSatisfied` proactive warning |

## Acceptance Criteria

- [ ] `synapse backend clients` shows "Authentication required" and "Cookie access_token (required)" in help
- [ ] `synapse backend clients list` (no token) shows "Authentication required. Use --token..." and exits non-zero
- [ ] `SYNAPSE_TOKEN=<jwt> synapse backend clients list` authenticates successfully
- [ ] `synapse --token <jwt> backend clients list` continues to work (existing behavior)
- [ ] `synapse --cookie access_token=<jwt> backend clients list` authenticates successfully
- [ ] `SYNAPSE_COOKIE_ACCESS_TOKEN=<jwt> synapse backend clients list` authenticates successfully
- [ ] `synapse backend forms list` (no `psRequest`) unaffected — no auth notice shown
- [ ] Origin (CORS): synapse sends no Origin header, `ValidOrigin` passes, no change needed
- [ ] `#[from_request]` fields do not appear in synapse's method parameter help (stripped by macro)
- [ ] Wire schema `request` field is an opaque JSON Schema blob — no Rust type strings, no `SecurityExtractor`, no `ContractEntry`
- [ ] Old backends without `request` field in `PluginSchema` are handled gracefully (`psRequest = Nothing`)

## Tests

### Haskell unit tests — `synapse/test/`

**`PluginSchema` JSON roundtrip:**
```haskell
describe "PluginSchema JSON roundtrip" $ do
  it "decodes psRequest when present" $ do
    let json = [aesonQQ| {
          "namespace": "clients", "version": "1.0.0", "description": "",
          "methods": [], "hash": "abc",
          "request": {
            "type": "object",
            "properties": {
              "auth_token": {
                "type": "string",
                "x-plexus-source": { "from": "cookie", "key": "access_token" }
              }
            },
            "required": ["auth_token"]
          }
        } |]
    let schema = fromJust (decode json) :: PluginSchema
    psRequest schema `shouldSatisfy` isJust
    let reqSchema = fromJust (psRequest schema)
    reqSchema ^? key "properties" . key "auth_token" `shouldSatisfy` isJust

  it "psRequest is Nothing when field absent" $ do
    let json = [aesonQQ| {"namespace":"x","version":"1","description":"","methods":[],"hash":"a"} |]
    let schema = fromJust (decode json) :: PluginSchema
    psRequest schema `shouldBe` Nothing

  it "roundtrip is lossless" $ do
    let schema = PluginSchema "ns" "1.0" "desc" [] "hash" (Just (object ["type" .= ("object" :: Text)]))
    decode (encode schema) `shouldBe` Just schema
```

**`renderRequestSchema`:**
```haskell
describe "renderRequestSchema" $ do
  it "renders Cookie access_token (required)" $ do
    let reqSchema = object
          [ "properties" .= object
              [ "auth_token" .= object
                  [ "type" .= ("string" :: Text)
                  , "description" .= ("JWT from Keycloak" :: Text)
                  , "x-plexus-source" .= object ["from" .= ("cookie" :: Text), "key" .= ("access_token" :: Text)]
                  ]
              ]
          , "required" .= ["auth_token" :: Text]
          ]
    let out = renderRequestSchema reqSchema
    out `shouldContain` "Cookie access_token (required)"
    out `shouldContain` "JWT from Keycloak"

  it "renders Server-derived for derived fields" $ do
    let reqSchema = object
          [ "properties" .= object
              [ "peer_addr" .= object
                  [ "x-plexus-source" .= object ["from" .= ("derived" :: Text)]
                  ]
              ]
          , "required" .= ([] :: [Text])
          ]
    renderRequestSchema reqSchema `shouldContain` "Server-derived"

  it "returns empty for no properties" $ do
    renderRequestSchema (object []) `shouldBe` ""
```

**`resolveToken` — token resolution priority:**
```haskell
describe "resolveToken" $ do
  it "SYNAPSE_TOKEN used when no --token flag" $
    withEnv [("SYNAPSE_TOKEN", "env-jwt")] $ do
      result <- resolveToken defaultOpts "mybackend"
      result `shouldBe` Just "env-jwt"

  it "--token flag takes priority over SYNAPSE_TOKEN" $
    withEnv [("SYNAPSE_TOKEN", "env-jwt")] $ do
      result <- resolveToken (defaultOpts { soToken = Just "flag-jwt" }) "mybackend"
      result `shouldBe` Just "flag-jwt"

  it "SYNAPSE_COOKIE_ACCESS_TOKEN used when SYNAPSE_TOKEN absent" $
    withEnv [("SYNAPSE_COOKIE_ACCESS_TOKEN", "cookie-jwt")] $ do
      result <- resolveToken defaultOpts "mybackend"
      result `shouldBe` Just "cookie-jwt"

  it "SYNAPSE_TOKEN takes priority over SYNAPSE_COOKIE_ACCESS_TOKEN" $
    withEnv [("SYNAPSE_TOKEN", "tok"), ("SYNAPSE_COOKIE_ACCESS_TOKEN", "other")] $ do
      result <- resolveToken defaultOpts "mybackend"
      result `shouldBe` Just "tok"

  it "falls through to file when env absent" $
    withEnv [] $ do
      result <- resolveToken defaultOpts "nonexistent-backend"
      result `shouldBe` Nothing
```

**`renderRpcError` — -32001 hint:**
```haskell
describe "renderRpcError" $ do
  it "includes --token hint on -32001" $ do
    let out = renderRpcError (-32001) "Authentication required: no token"
    out `shouldContain` "--token"
    out `shouldContain` "SYNAPSE_TOKEN"
    out `shouldContain` "Authentication required"

  it "renders other errors without token hint" $ do
    let out = renderRpcError (-32000) "Execution error"
    out `shouldNotContain` "SYNAPSE_TOKEN"
    out `shouldContain` "Execution error"
```

### CLI integration tests — `synapse/test/cli-test/` (requires running backend)

Backend must have a `clients` hub with `request = ClientsRequest` (REQ-4) and a `forms` hub with no request struct.

```
-- Auth notice in hub help:
synapse backend clients
  stdout must contain "Authentication required"
  stdout must contain "Cookie access_token (required)"

-- No auth notice for unsecured hub:
synapse backend forms
  stdout must NOT contain "Authentication required"
  stdout must NOT contain "Request requirements"

-- -32001 error with hint:
synapse backend clients list   (no token)
  stderr must contain "Authentication required"
  stderr must contain "--token"
  exit code non-zero

-- SYNAPSE_TOKEN:
SYNAPSE_TOKEN=<valid-jwt> synapse backend clients list
  exit code 0
  stdout contains client data

-- --token flag (existing):
synapse --token <valid-jwt> backend clients list
  exit code 0

-- --cookie flag:
synapse --cookie access_token=<valid-jwt> backend clients list
  exit code 0

-- SYNAPSE_COOKIE_ACCESS_TOKEN:
SYNAPSE_COOKIE_ACCESS_TOKEN=<valid-jwt> synapse backend clients list
  exit code 0

-- CORS: no Origin header from synapse → ValidOrigin passes:
synapse --token <valid-jwt> backend clients list
  exit code 0   (server has origin: ValidOrigin in request struct)

-- Method params stripped: #[from_request] fields absent from help:
synapse --token <valid-jwt> backend clients list --help
  stdout must NOT contain "auth_token" as a parameter name
  stdout must NOT contain "peer_addr" as a parameter name
  stdout must NOT contain "origin" as a parameter name
  (these are request struct fields, not RPC params)
```

## Open Design Questions

**Q1: Should synapse validate the value format of request fields (e.g. check auth_token looks like a JWT)?**

No for v1. The JSON Schema `format` field can express this but synapse doesn't need to validate — the server will reject bad values with a meaningful error. Proactive format validation can be added if it proves useful.

**Q2: What if `psRequest` is present but `x-plexus-source` is absent on some fields?**

Treat as `derived` — synapse cannot help populate them, but shows them in docs as "Unknown (optional): field_name". The server may compute them server-side or they may be internal fields (`#[from_auth_context]`). Either way, synapse can't supply them.

**Q3: Should `SYNAPSE_TOKEN` be deprecated in favor of `SYNAPSE_COOKIE_ACCESS_TOKEN`?**

No. Keep both. `SYNAPSE_TOKEN` is a better UX shortcut for the common case. `SYNAPSE_COOKIE_ACCESS_TOKEN` is the general form that makes the transport mechanism explicit. Both map to the same `access_token` cookie on WS upgrade.
