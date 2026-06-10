-- Minimal YAML subset for crabcraft manifests (docs/WIRE.md section 4):
-- nested maps by 2-space indentation, lists of scalars ("- item"), scalars
-- (plain / 'single' / "double" quoted, numbers, true/false), comments (#).
-- No anchors, no multi-doc, no block scalars, no flow collections.
local M = {}

local function parse_scalar(s)
  s = s:match("^%s*(.-)%s*$")
  local q = s:match('^"(.*)"$') or s:match("^'(.*)'$")
  if q then return q end
  if s == "true" then return true end
  if s == "false" then return false end
  if s == "null" or s == "~" or s == "" then return nil end
  local n = tonumber(s)
  if n then return n end
  return s
end

function M.decode(text)
  local lines = {}
  for line in (text .. "\n"):gmatch("(.-)\n") do
    local stripped = line:gsub("#.*$", "")
    if stripped:match("%S") then
      local indent = #(stripped:match("^( *)"))
      lines[#lines + 1] = { indent = indent, body = stripped:sub(indent + 1) }
    end
  end

  local pos = 0
  local function parse_block(indent)
    local node
    while pos < #lines do
      local ln = lines[pos + 1]
      if ln.indent < indent then break end
      if ln.indent > indent then error("yaml: bad indentation at '" .. ln.body .. "'") end
      if ln.body:match("^%- ") or ln.body == "-" then
        node = node or {}
        if type(node) ~= "table" then error("yaml: mixed list/map") end
        pos = pos + 1
        local item = ln.body:sub(3)
        if item:match("%S") then
          node[#node + 1] = parse_scalar(item)
        else
          node[#node + 1] = parse_block(indent + 2)
        end
      else
        local key, rest = ln.body:match("^([%w%-%._/]+):%s*(.*)$")
        if not key then error("yaml: cannot parse line '" .. ln.body .. "'") end
        node = node or {}
        pos = pos + 1
        if rest:match("%S") then
          node[key] = parse_scalar(rest)
        else
          -- nested block (or empty value)
          local nxt = lines[pos + 1]
          if nxt and nxt.indent > indent then
            node[key] = parse_block(nxt.indent)
          else
            node[key] = nil
          end
        end
      end
    end
    return node or {}
  end

  return parse_block(lines[1] and lines[1].indent or 0)
end

return M
