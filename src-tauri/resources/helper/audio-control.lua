-- Core.require_api and the Lua proxy API are shared by WirePlumber 0.4/0.5.
-- The request is a bounded, byte-escaped Lua table prepended by the Rust helper.
local done = false
local deadline = GLib.get_monotonic_time() + 4000000
local selected, device, metadata, mixer
local device_identity
local node_om, device_om, metadata_om
local configured_key = "default.configured.audio.sink"
local effective_key = "default.audio.sink"
local level_phases, phase_index, phase_before, level_guard
local confirm, write_level_phase

local function to_json_str(obj)
  return obj.to_string and obj:to_string() or obj:get_data()
end

local function finish(code, message)
  if done then return end
  done = true
  local outcome = code and Json.Object {
    status = "error", code = code, message = message,
  } or Json.Object { status = "confirmed" }
  print(to_json_str(Json.Object { schemaVersion = 1, outcome = outcome }))
  Core.quit()
end

local function fail(message)
  finish("notConfirmed", message)
  return false
end

local function bound(proxy)
  return proxy and (proxy:get_active_features() & Feature.Proxy.BOUND) ~= 0
end

local function cookie_matches()
  local info = Core.get_info()
  return info and info.name and info.version and tostring(info.cookie) == request.cookie
end

local function same_node()
  if not request.node then return true end
  if not bound(selected) then return false end
  local properties = selected.properties
  if selected["bound-id"] ~= request.node.id or
      properties["object.serial"] ~= request.node.serial or
      properties["node.name"] ~= request.node.name or
      properties["media.class"] ~= request.node.class then return false end
  if request.node.session and (properties["node.virtual"] ~= "true" or
      properties["peripheral-hub.session"] ~= request.node.session or properties["device.id"]) then return false end
  local count = 0
  for node in node_om:iterate() do
    if node.properties["node.name"] == request.node.name then
      if node ~= selected then return false end
      count = count + 1
    end
  end
  return count == 1
end

local function same_device()
  if not device_identity then return true end
  if not bound(device) then return false end
  local properties = device.properties
  return device_om:lookup {
    type = "device",
    Constraint { "bound-id", "equals", device_identity.id, type = "gobject" },
  } == device and device["bound-id"] == device_identity.id and
    properties["object.serial"] == device_identity.serial and
    properties["device.name"] == device_identity.name and
    properties["media.class"] == "Audio/Device" and
    selected.properties["device.id"] == device_identity.reference and
    selected.properties["card.profile.device"] == device_identity.profile
end

local function validate_identity()
  return cookie_matches() and same_node() and same_device()
end

local function default_name(key)
  if not bound(metadata) then return nil, false end
  local value, value_type = metadata:find(0, key)
  if value == nil then return nil, value_type == nil end
  if value_type ~= "Spa:String:JSON" or type(value) ~= "string" or #value > 4096 then return nil, false end
  -- Two bounded levels distinguish a string name from an array/object name.
  local ok, parsed = pcall(function()
    local json = Json.Raw(value)
    return json and json:is_object() and json:parse(2)
  end)
  if not ok or type(parsed) ~= "table" or type(parsed.name) ~= "string" or
      #parsed.name == 0 or #parsed.name > 512 or parsed.name:find("%c") then
    return nil, false
  end
  return parsed.name, true
end

local function metadata_matches()
  return bound(metadata) and metadata_om:lookup { type = "metadata" } == metadata and
    metadata_om:get_n_objects() == 1
end

local function eq_route_matches()
  if not metadata_matches() then return false end
  local configured, configured_valid = default_name(configured_key)
  local effective, effective_valid = default_name(effective_key)
  local expected = request.node.name
  return configured_valid and effective_valid and configured == expected and effective == expected
end

-- Only the filter-chain's exported DSP parameters, never volume or mute.
local function read_eq_controls()
  local values = {}
  for pod in selected:iterate_params("Props") do
    local parsed = pod:parse()
    local params = parsed.properties and parsed.properties.params
    if type(params) == "table" then
      for i = 1, #params, 2 do
        local name, value = params[i], params[i + 1]
        if type(name) == "string" and type(value) == "number" and value == value and math.abs(value) < math.huge then
          if values[name] ~= nil then return nil end
          values[name] = value
        end
      end
    end
  end
  return values
end

local function volume_matches(level, volume)
  if type(level) ~= "table" then return false end
  if type(level.volume) ~= "number" or level.volume ~= level.volume or
      math.abs(level.volume - volume) > 0.002 then return false end
  local count = 0
  for _, channel in pairs(level.channelVolumes or {}) do
    count = count + 1
    if count > 64 or type(channel) ~= "table" or type(channel.volume) ~= "number" or
        channel.volume ~= channel.volume or math.abs(channel.volume - volume) > 0.002 then return false end
  end
  -- Scalar Props volume is valid too; wpctl supports nodes with no channelVolumes.
  return true
end

local function same_volume(level, previous)
  if type(level.volume) ~= "number" or level.volume ~= level.volume or
      math.abs(level.volume - previous.volume) > 0.002 then return false end
  local count, old_count = 0, 0
  for key, channel in pairs(level.channelVolumes or {}) do
    count = count + 1
    local old = (previous.channelVolumes or {})[key]
    if count > 64 or type(channel) ~= "table" or type(old) ~= "table" or
        type(channel.volume) ~= "number" or channel.volume ~= channel.volume or
        math.abs(channel.volume - old.volume) > 0.002 then return false end
  end
  for _ in pairs(previous.channelVolumes or {}) do old_count = old_count + 1 end
  return count == old_count
end

local function read_level()
  local level = mixer:call("get-volume", selected["bound-id"])
  if type(level) ~= "table" or type(level.volume) ~= "number" or
      level.volume ~= level.volume or math.abs(level.volume) == math.huge or
      type(level.mute) ~= "boolean" or
      (level.channelVolumes ~= nil and type(level.channelVolumes) ~= "table") then return nil end
  if not same_volume(level, level) then return nil end
  for _, channel in pairs(level.channelVolumes or {}) do
    if math.abs(channel.volume) == math.huge then return nil end
  end
  return level
end

local function sync_confirm()
  Core.sync(function(error)
    if done then return end
    if error then fail("Sincronizzazione della scrittura fallita."); return end
    if not validate_identity() then fail("Proxy cambiato dopo la sincronizzazione."); return end
    -- mixer-api refreshes Route/Props after its own core sync. Recheck the held
    -- node AND Device proxy after each barrier, not just the last readback.
    Core.sync(function(error)
      if done then return end
      if error then fail("Sincronizzazione del readback fallita."); return end
      if not validate_identity() then fail("Proxy cambiato dopo il readback sync."); return end
      Core.idle_add(function() confirm(); return false end)
    end)
  end)
end

confirm = function()
  if done then return end
  if not validate_identity() then
    fail("Identita del proxy cambiata durante la conferma.")
    return
  end
  local confirmed
  if request.operation == "setLevel" then
    local level = read_level()
    if not level or not validate_identity() then fail("Proxy/cache del livello non valido."); return end
    local phase = level_phases[phase_index]
    if #level_phases > 1 then
      -- While changing one field, its confirmed safety prerequisite must not
      -- drift: mute stays asserted for a gain raise; gain stays fixed for unmute.
      if (phase.volume ~= nil and level.mute ~= phase_before.mute) or
          (phase.mute ~= nil and not same_volume(level, phase_before)) then
        fail("Prerequisito del livello cambiato esternamente fra le fasi.")
        return
      end
    end
    confirmed = phase.volume ~= nil and volume_matches(level, phase.volume) or
      phase.mute ~= nil and level.mute == phase.mute
    if not confirmed and phase.volume ~= nil and not same_volume(level, phase_before) then
      fail("Volume cambiato esternamente durante la conferma.")
      return
    end
    if confirmed then
      level_guard = level
      if phase_index < #level_phases then
        phase_index = phase_index + 1
        Core.idle_add(function() write_level_phase(); return false end)
        return
      end
      if (request.volume ~= nil and not volume_matches(level, request.volume)) or
          (request.muted ~= nil and level.mute ~= request.muted) then
        fail("Stato completo del livello non confermato.")
        return
      end
    end
  elseif request.operation == "setEqualizer" then
    if not eq_route_matches() then fail("Uscita cambiata durante la conferma EQ."); return end
    local values = read_eq_controls()
    confirmed = values ~= nil
    for i = 1, #request.controls, 2 do
      local name, expected = request.controls[i], request.controls[i + 1]
      local value = values and values[name]
      if not value or math.abs(value - expected) > math.max(math.abs(expected) * 5e-6, 1e-8) then
        confirmed = false
        break
      end
    end
  else
    if not metadata_matches() then fail("Proxy Metadata sostituito."); return end
    local name, valid = default_name(configured_key)
    if not valid then fail("Default configurato non leggibile."); return end
    confirmed = name == (request.node and request.node.name or nil)
    if not confirmed and name ~= request.expected_configured then
      fail("Scelta esterna del default osservata; nessuna riscrittura.")
      return
    end
  end
  if confirmed then finish(); return end
  if GLib.get_monotonic_time() >= deadline then
    fail("Readback dei parametri audio non confermato.")
    return
  end
  Core.timeout_add(50, function()
    if not done then sync_confirm() end
    return false
  end)
end

write_level_phase = function()
  if done then return end
  if not validate_identity() then fail("Identita PipeWire cambiata prima della fase."); return end
  local level = read_level()
  if not level then finish("unsupported", "Il nodo/Device Route non espone controlli di volume."); return end
  if #level_phases > 1 and (level.mute ~= level_guard.mute or not same_volume(level, level_guard)) then
    fail("Stato confermato cambiato prima della fase successiva.")
    return
  end
  phase_before = level
  local phase = level_phases[phase_index]
  -- No yield between validation of the shared WpCore proxy/cache and the
  -- synchronous mixer lookup/set. Nil fields are omitted, never parent snapshots.
  -- mixer-api retains unrequested Props from its current cache, as wpctl does;
  -- neither API provides atomic server-side field CAS against external writers.
  if not validate_identity() then fail("Proxy/cache cambiato prima della scrittura."); return end
  if not mixer:call("set-volume", selected["bound-id"], {
      volume = phase.volume, mute = phase.mute,
    }) then
    finish("unsupported", "Scrittura mixer-api rifiutata.")
    return
  end
  sync_confirm()
end

local function commit()
  if done then return false end
  if not validate_identity() then fail("Identita PipeWire non corrispondente alla richiesta."); return false end
  if request.operation == "setLevel" then
    local cached = read_level()
    if not cached then
      finish("unsupported", "Il nodo/Device Route non espone controlli di volume.")
      return false
    end
    -- ACP applies mute immediately but channelVolumes at the end of its work.
    -- Never combine both requested fields, even if both are in the parent API.
    if request.volume ~= nil and request.muted ~= nil then
      if request.muted then
        level_phases = { { mute = true }, { volume = request.volume } }
      else
        level_phases = { { volume = request.volume }, { mute = false } }
      end
    else
      level_phases = { { volume = request.volume, mute = request.muted } }
    end
    phase_index, level_guard = 1, cached
    write_level_phase()
    return false
  elseif request.operation == "setEqualizer" then
    if not eq_route_matches() then fail("Uscita cambiata prima della regolazione EQ."); return false end
    local values = read_eq_controls()
    for i = 1, #request.controls, 2 do
      if not values or values[request.controls[i]] == nil then
        finish("unsupported", "Il filtro non espone i controlli dell'EQ.")
        return false
      end
    end
    if not validate_identity() or not eq_route_matches() then
      fail("Identità/uscita cambiata prima della scrittura EQ."); return false
    end
    local ok = pcall(function()
      local params = {}
      for i = 1, #request.controls, 2 do
        params[i], params[i + 1] = request.controls[i], Pod.Float(request.controls[i + 1])
      end
      selected:set_param("Props", Pod.Object { "Spa:Pod:Object:Param:Props", "Props", params = Pod.Struct(params) })
    end)
    if not ok then fail("Impossibile scrivere i parametri del filtro EQ."); return false end
  else
    if not metadata_matches() then fail("Proxy Metadata non univoco."); return false end
    local configured, configured_valid = default_name(configured_key)
    local effective, effective_valid = default_name(effective_key)
    if not configured_valid or not effective_valid then
      fail("Baseline Metadata non leggibile; nessuna scrittura.")
      return false
    end
    if configured ~= request.expected_configured or effective ~= request.expected_effective then
      fail("Baseline del default cambiato; scelta esterna conservata.")
      return false
    end
    -- No yield between the two client-cache reads and the write; this is NOT server-side CAS.
    -- Do not call default-nodes-api's unescaped JSON setter (0.4/0.5).
    -- The name is the original requested name, not a new numeric-ID lookup.
    if not validate_identity() or not metadata_matches() then
      fail("Proxy cambiato prima del default."); return false
    end
    metadata:set(0, configured_key, request.node and "Spa:String:JSON" or nil, request.name_json)
  end
  sync_confirm()
  return false
end

local function prepare()
  if done then return end
  if not cookie_matches() then fail("Cookie del server PipeWire cambiato."); return end
  if request.node then
    for node in node_om:iterate() do
      if node.properties["node.name"] == request.node.name then
        if selected then fail("Nome del nodo non univoco."); return end
        selected = node
      end
    end
    if not same_node() then finish("targetUnavailable", "Istanza del nodo audio non disponibile."); return end
    if request.operation == "setLevel" then
      local properties = selected.properties
      local reference = properties["device.id"]
      if reference then
        if not reference:match("^%d+$") then fail("Riferimento Device non valido."); return end
        device = device_om:lookup {
          type = "device",
          Constraint { "bound-id", "equals", tonumber(reference), type = "gobject" },
        }
        if not bound(device) or not device.properties["object.serial"] or
            not device.properties["device.name"] or device.properties["media.class"] ~= "Audio/Device" then
          fail("Istanza Device non disponibile; nessun fallback su ID.")
          return
        end
        device_identity = {
          id = device["bound-id"], serial = device.properties["object.serial"],
          name = device.properties["device.name"], reference = reference,
          profile = properties["card.profile.device"],
        }
      end
    end
  end
  if request.operation == "setDefault" or request.operation == "setEqualizer" then
    metadata = metadata_om:lookup { type = "metadata" }
    if not metadata_matches() then finish("audioUnavailable", "Metadata default assente o ambiguo."); return end
  end
  Core.sync(function(error)
    if error then fail("Sincronizzazione del proxy fallita."); return end
    Core.idle_add(commit)
  end)
end

local function start(api)
  mixer = api
  if mixer then mixer.scale = "cubic" end
  node_om = ObjectManager { Interest {
    type = "node", Constraint { "media.class", "matches", "*Audio*", type = "pw-global" },
  } }
  device_om = ObjectManager { Interest {
    type = "device", Constraint { "media.class", "equals", "Audio/Device", type = "pw-global" },
  } }
  metadata_om = ObjectManager { Interest {
    type = "metadata", Constraint { "metadata.name", "equals", "default", type = "pw-global" },
  } }
  local installed = 0
  local function ready()
    installed = installed + 1
    if installed == 3 then prepare() end
  end
  node_om:connect("installed", ready)
  device_om:connect("installed", ready)
  metadata_om:connect("installed", ready)
  node_om:activate()
  device_om:activate()
  metadata_om:activate()
end

Core.timeout_add(4000, function()
  finish("timeout", "Deadline del client WirePlumber superata.")
  return false
end)
if request.operation == "setLevel" then
  Core.require_api("mixer", start)
else
  start(nil)
end
