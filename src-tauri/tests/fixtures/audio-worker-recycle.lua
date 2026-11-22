-- Real null-sink ID recycling on one isolated connection, no CLI reconnect.
local old, replacement
local before
local function to_json_str(obj)
  return obj.to_string and obj:to_string() or obj:get_data()
end
local function abort(error)
  print(to_json_str(Json.Object { error = tostring(error) }))
  Core.quit()
end
local function identity(node)
  return Json.Object {
    id = node["bound-id"], serial = node.properties["object.serial"],
    name = node.properties["node.name"], class = node.properties["media.class"],
  }
end
local function new_node(name)
  return Node("adapter", {
    ["factory.name"] = "support.null-audio-sink",
    ["node.name"] = name,
    ["media.class"] = "Audio/Sink",
    ["audio.channels"] = "2",
    ["audio.position"] = "FL,FR",
    ["object.linger"] = "true",
  })
end
old = new_node("worker.recycled.old")
old:activate(Features.ALL, function(node, error)
  if error then abort(error); return end
  before = identity(node)
  node:request_destroy()
  Core.sync(function(error)
    if error then abort(error); return end
    Core.sync(function(error)
      if error then abort(error); return end
      replacement = new_node("worker.recycled.old")
      replacement:activate(Features.ALL, function(node, error)
        if error then abort(error); return end
        print(to_json_str(Json.Object {
          cookie = tostring(Core.get_info().cookie), before = before, after = identity(node),
        }))
        Core.quit()
      end)
    end)
  end)
end)
Core.timeout_add(4000, function() abort("fixture deadline"); return false end)
