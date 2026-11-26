-- Deterministic fake WirePlumber main loop. No sockets, devices or host audio.
local clock, queue, sequence, quit = 0, {}, 0, false
local calls, writes, syncs, hazard = 0, 0, 0, false
local replacement
local node_name = request.node and request.node.name or "none"
local original = { ["bound-id"] = request.node and request.node.id or 30, features = 1,
  properties = { ["node.name"] = node_name, ["media.class"] = "Audio/Sink",
    ["object.serial"] = request.node and request.node.serial or "1" } }
local nodes = { original }
if scenario == "serial-mismatch" then original.properties["object.serial"] = "2" end
if scenario == "duplicate-node" then
  nodes[2] = { ["bound-id"] = 31, features = 1, properties = original.properties }
end
local original_device = { ["bound-id"] = 20, features = 1,
  properties = { ["object.serial"] = "100", ["device.name"] = "stable-card", ["media.class"] = "Audio/Device" } }
local devices = {}
local acp = scenario:match("^acp%-") ~= nil
if scenario == "device-route" or scenario == "device-replace" or acp then
  original.properties["device.id"] = "20"
  original.properties["card.profile.device"] = "0"
  devices[1] = original_device
end
local function quote(value)
  return '"' .. value:gsub('[%z\1-\31\\"]', function(char)
    if char == '\\' or char == '"' then return '\\' .. char end
    return string.format('\\u%04x', string.byte(char))
  end) .. '"'
end
local decoded = { ['{"name":[]}'] = { name = {} } }
local function default_value(name)
  if name == nil then return nil end
  local value = '{"name":' .. quote(name) .. '}'
  decoded[value] = { name = name }
  return value
end
local configured_key, effective_key = "default.configured.audio.sink", "default.audio.sink"
local metadata_values = {
  [configured_key] = default_value(request.expected_configured),
  [effective_key] = default_value(request.expected_effective),
}
if scenario == "malformed-default" then metadata_values[configured_key] = "[]" end
if scenario == "structured-default" then metadata_values[configured_key] = '{"name":[]}' end
local turn, metadata_reads = 0, {}
local meta = { features = 1, ["bound-id"] = 10 }
local metadata_objects = { meta }
local volume, muted, channel = 0.8, false, 0.8
local hardware_volume, hardware_muted = volume, muted
local gain_ack, mute_ack = false, false
local volume_calls, mute_calls = 0, 0
local trace = {}
local drifted, cookie_changed = false, false
if acp then
  volume = request.muted and 0.1 or 1.0
  channel = volume
  muted = not request.muted
  hardware_volume, hardware_muted = volume, muted
end
local function proxy_features(self) return self.features end
original.get_active_features = proxy_features
original_device.get_active_features = proxy_features
meta.get_active_features = proxy_features

local function replace_node()
  original.features = 0
  replacement = { ["bound-id"] = original["bound-id"], features = 1,
    properties = { ["node.name"] = "wrong-recycled-instance", ["media.class"] = "Audio/Source", ["object.serial"] = "2" },
    get_active_features = proxy_features }
  nodes = { replacement }
end
local function replace_device()
  original_device.features = 0
  devices = { { ["bound-id"] = 20, features = 1,
    properties = { ["object.serial"] = "200", ["device.name"] = "other-card", ["media.class"] = "Audio/Device" },
    get_active_features = proxy_features } }
end
local function phase_drift()
  if drifted or calls ~= 1 or not (gain_ack or mute_ack) then return end
  if scenario == "acp-node-swap" or scenario == "acp-node-swap-muted" then replace_node()
  elseif scenario == "acp-device-swap" or scenario == "acp-device-swap-muted" then replace_device()
  elseif scenario == "acp-cookie-swap" or scenario == "acp-cookie-swap-muted" then cookie_changed = true
  elseif scenario == "acp-volume-drift" then
    volume, channel, hardware_volume = 1.0, 1.0, 1.0
  elseif scenario == "acp-mute-drift" then
    muted, hardware_muted = false, false
  else return end
  drifted = true
end
local function schedule(delay, callback)
  sequence = sequence + 1
  queue[#queue + 1] = { due = clock + delay * 1000, callback = callback, sequence = sequence }
end
GLib = { get_monotonic_time = function() return clock end }
Feature = { Proxy = { BOUND = 1 } }
Core = {
  get_info = function()
    return { name = "isolated-fake", version = "1",
      cookie = (scenario == "cookie-mismatch" or cookie_changed) and 1 or tonumber(request.cookie) }
  end,
  require_api = function(_, callback) schedule(0, function() callback(test_mixer) end) end,
  idle_add = function(callback)
    if hazard and scenario == "no-yield" then replace_node() end
    schedule(0, function() phase_drift(); callback() end)
  end,
  timeout_add = function(delay, callback)
    if hazard and scenario == "no-yield" then replace_node() end
    schedule(delay, callback)
  end,
  sync = function(callback)
    if hazard and scenario == "no-yield" then replace_node() end
    syncs = syncs + 1
    local current = syncs
    schedule(0, function()
      if current == 1 then
        if scenario == "volume-only" then volume = 0.6; channel = 0.6; muted = true end
        if scenario == "mute-only" then volume = 0.6; channel = 0.6; muted = false end
        if scenario == "replace-before-write" then replace_node() end
        if scenario == "device-replace" then
          replace_device()
        end
        if scenario == "baseline-change" then metadata_values[configured_key] = default_value("external-choice") end
        if scenario == "effective-change" then metadata_values[effective_key] = default_value("external-choice") end
        if scenario == "effective-removed" then metadata_values[effective_key] = nil end
        if scenario == "malformed-effective" then metadata_values[effective_key] = '{"name":' end
        if scenario == "metadata-replace" then meta.features = 0; metadata_objects = {} end
      elseif current == 2 and scenario == "replace-after-write" then replace_node() end
      if scenario == "acp-node-swap-sync" and calls == 1 then replace_node() end
      if scenario == "acp-device-swap-sync" and calls == 1 then replace_device() end
      if scenario == "acp-volume-drift-final" and calls == 2 then
        volume, channel, hardware_volume = 1.0, 1.0, 1.0
      end
      callback(nil)
    end)
  end,
  quit = function() quit = true end,
}
Constraint = function(spec) return spec end
Interest = function(spec) return spec end
ObjectManager = function(interests)
  local kind = interests[1].type
  local handlers = {}
  local function objects()
    return kind == "node" and nodes or kind == "device" and devices or metadata_objects
  end
  return {
    connect = function(_, signal, callback) handlers[signal] = callback end,
    activate = function(self) schedule(0, function() handlers.installed(self) end) end,
    get_n_objects = function() return #objects() end,
    lookup = function(_, interest)
      local id = interest and interest[1] and interest[1][3]
      for _, object in ipairs(objects()) do
        if not id or object["bound-id"] == id then return object end
      end
    end,
    iterate = function()
      local index = 0
      return function() index = index + 1; return objects()[index] end
    end,
  }
end

local function encode(value)
  if type(value) == "string" then return quote(value) end
  if type(value) == "number" or type(value) == "boolean" then return tostring(value) end
  local fields = {}
  for key, item in pairs(value.fields) do fields[#fields + 1] = quote(key) .. ":" .. encode(item) end
  return "{" .. table.concat(fields, ",") .. "}"
end
Json = {
  Object = function(fields) return { fields = fields, to_string = function(self) return encode(self) end } end,
  Raw = function(value)
    return { is_object = function() return value:sub(1, 1) == "{" end,
      parse = function(_, depth)
        assert(depth == 2)
        return assert(decoded[value], "malformed fixture JSON")
      end }
  end,
}
meta.find = function(self, subject, key)
  assert(self == meta and subject == 0 and (key == configured_key or key == effective_key))
  metadata_reads[key] = turn
  local value = metadata_values[key]
  return value, value and "Spa:String:JSON" or nil
end
meta.set = function(self, subject, key, value_type, value)
  assert(self == meta and subject == 0 and key == configured_key)
  assert(meta.features == 1)
  assert(metadata_reads[configured_key] == turn and metadata_reads[effective_key] == turn,
    "both default baselines must be read on the write turn")
  assert(value == default_value(request.node and request.node.name))
  assert(value_type == (request.node and "Spa:String:JSON" or nil))
  writes = writes + 1
  metadata_values[key] = value
end
test_mixer = {
  call = function(_, action, id, parameters)
    assert(id == original["bound-id"])
    if action == "get-volume" then
      if calls == 0 then
        hazard = true
        if scenario == "cache-side-effect" then replace_node() end
      end
      if acp then
        if volume_calls > 0 and math.abs(volume - request.volume) <= 0.002 and
            math.abs(hardware_volume - request.volume) <= 0.002 then gain_ack = true end
        if mute_calls > 0 and muted == true and hardware_muted == true then mute_ack = true end
      end
      return { id = id, volume = volume, mute = muted,
        channelVolumes = scenario == "scalar-volume" and {} or
          { ["0"] = { volume = volume }, ["1"] = { volume = channel } } }
    end
    assert(action == "set-volume" and original.features == 1 and nodes[1] == original)
    if parameters.volume ~= nil then assert(math.type(parameters.volume) == "float") end
    if scenario == "volume-only" then assert(parameters.mute == nil) end
    if scenario == "mute-only" then assert(parameters.volume == nil) end
    assert(test_mixer.scale == "cubic")
    if scenario == "device-route" then assert(devices[1] == original_device) end
    if acp then
      assert(devices[1] == original_device and original_device.features == 1 and not cookie_changed,
        "same bound node/device/cookie at each phase write")
      local native_mute = parameters.mute
      if native_mute == nil then native_mute = muted end
      local native_volume = parameters.volume or volume
      -- ACP applies mute immediately, before its deferred channelVolumes work.
      if native_mute == false and request.volume ~= nil then
        assert(gain_ack and math.abs(hardware_volume - request.volume) <= 0.002,
          "ACP unmute before confirmed gain reduction")
      end
      hardware_muted = native_mute
      if request.muted == true and native_volume > hardware_volume then
        assert(hardware_muted and mute_ack, "ACP gain raise before mute acknowledgement")
      end
      assert((parameters.volume == nil) ~= (parameters.mute == nil),
        "mixer call must contain exactly one requested field")
      trace[#trace + 1] = { volume = parameters.volume, mute = parameters.mute,
        proxy = original, device = devices[1], gain_ack = gain_ack, mute_ack = mute_ack }
      if parameters.volume ~= nil then volume_calls = volume_calls + 1 end
      if parameters.mute ~= nil then mute_calls = mute_calls + 1 end
      if scenario ~= "acp-mute-ack-fail" then muted = native_mute end
      schedule(30, function()
        hardware_volume = native_volume
        if scenario ~= "acp-volume-ack-fail" then volume, channel = native_volume, native_volume end
      end)
      calls = calls + 1
      hazard = false
      return true
    end
    assert((parameters.volume == nil) ~= (parameters.mute == nil),
      "mixer call must contain exactly one requested field")
    calls = calls + 1
    hazard = false
    if parameters.volume ~= nil then
      if scenario ~= "wrong-level" then volume = parameters.volume end
      if scenario ~= "channel-mismatch" then channel = parameters.volume end
    end
    if parameters.mute ~= nil and scenario ~= "wrong-mute" then muted = parameters.mute end
    return true
  end,
}

function worker_test_finish()
  local iterations = 0
  while not quit and #queue > 0 do
    table.sort(queue, function(left, right)
      return left.due == right.due and left.sequence < right.sequence or left.due < right.due
    end)
    local event = table.remove(queue, 1)
    clock, turn = event.due, event.sequence
    event.callback()
    iterations = iterations + 1
    assert(iterations < 1000)
  end
  assert(quit, "script did not finish")
  if acp then
    local success = scenario == "acp-lower-before-unmute" or scenario == "acp-mute-before-raise"
    local final_drift = scenario == "acp-volume-drift-final"
    if success or final_drift then
      assert(calls == 2 and volume_calls == 1 and mute_calls == 1, "one write per phase, no retries")
    else
      assert(calls == 1, "failed first phase must never advance")
      if request.muted then assert(volume_calls == 0) else assert(mute_calls == 0) end
    end
    for _, entry in ipairs(trace) do
      assert(entry.proxy == original and entry.device == original_device, "same held node/device per phase")
      assert((entry.volume == nil) ~= (entry.mute == nil), "no combined native mixer call")
    end
    if success then
      assert(math.abs(hardware_volume - request.volume) <= 0.002 and hardware_muted == request.muted)
      if request.muted then assert(trace[1].mute == true and trace[2].mute_ack)
      else assert(trace[1].volume == request.volume and trace[2].gain_ack) end
    end
    if scenario == "acp-mute-ack-fail" then assert(not mute_ack and volume_calls == 0) end
    if scenario == "acp-volume-ack-fail" then assert(not gain_ack and mute_calls == 0) end
  else
    assert(calls <= 2 and writes <= 1, "readback must never retry mutations")
  end
  if scenario == "normal" or scenario == "scalar-volume" or scenario == "no-yield" or scenario == "device-route" then assert(calls == 2) end
  if scenario == "volume-only" or scenario == "mute-only" then assert(calls == 1) end
  if scenario == "wrong-level" or scenario == "channel-mismatch" then assert(calls == 2) end
  if scenario == "wrong-mute" then assert(calls == 1) end
  if scenario == "volume-only" then assert(muted == true and volume == 0.25) end
  if scenario == "mute-only" then assert(volume == 0.6 and muted == true) end
  if scenario == "serial-mismatch" or scenario == "replace-before-write" or scenario == "cache-side-effect" or scenario == "device-replace" or scenario == "cookie-mismatch" then
    assert(calls == 0 and writes == 0, "stale proxy must not mutate a recycled instance")
  end
  if scenario == "replace-after-write" then assert(calls == 1 and replacement.features == 1) end
  if request.operation == "setDefault" then
    if scenario == "default" or scenario == "clear" then assert(writes == 1)
    else assert(writes == 0, "external/invalid default must be preserved") end
  end
end
