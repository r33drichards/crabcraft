-- pets: crabcraft demo - drive the 'sqlite' workload from a plain Lua script.
-- Self-contained: fetches the client runtime (crblib) on first run, connects
-- to whatever gateway answers, builds a schema-driven proxy. No CLI involved.
--   wget https://github.com/r33drichards/crabcraft/releases/latest/download/pets.lua pets
--   pets
local LIBURL = "https://github.com/r33drichards/crabcraft/releases/latest/download/crblib.lua"
if not fs.exists("crblib") then
  io.write("fetching crblib ... ")
  local r = assert(http.get(LIBURL), "cannot fetch crblib")
  local h = fs.open("crblib", "w") h.write(r.readAll()) h.close() r.close()
  print("ok")
end
local lib = dofile("crblib")

local C = lib.client.connect()
local db = C:workload("sqlite")

local function run(sql)
  print("> " .. sql)
  local r = db.exec({ sql })
  if r.is_err then
    print("  SQL error: " .. tostring(r.err))
    return nil
  end
  local rows = lib.json.decode(r.ok)
  if rows.columns and #rows.columns > 0 then
    print("  " .. table.concat(rows.columns, " | "))
    for _, row in ipairs(rows.rows or {}) do
      local cells = {}
      for i, cell in ipairs(row) do cells[i] = tostring(cell) end
      print("  " .. table.concat(cells, " | "))
    end
  end
  print(("  (%s change(s))"):format(tostring(rows.changes)))
  return rows
end

run("CREATE TABLE IF NOT EXISTS pets(name,kind)")
run("INSERT INTO pets VALUES('ferris','crab'),('gopher','rodent')")
run("SELECT name, kind FROM pets ORDER BY name")
run("SELECT COUNT(*) AS n FROM pets")
