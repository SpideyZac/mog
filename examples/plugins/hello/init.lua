-- copy this folder into <config dir>/mog/plugins to try it

mog.on("open", function(path)
  if path ~= "" then
    mog.notify("mog is mogging " .. path)
  end
end)

mog.command("hello.wave", function()
  mog.notify("o/ from mog " .. mog.version)
end)
