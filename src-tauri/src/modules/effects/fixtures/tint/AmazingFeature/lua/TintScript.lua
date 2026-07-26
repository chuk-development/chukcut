---@class TintScript : ScriptComponent [UI(Display="Tint")]
---@field intensity double  [UI(Display="Strength", Range={0, 1}, Slider, Order=0)]
---@field speed     double  [UI(Range={0, 8}, Drag, Order=1)]
---@field blendMode string  [UI(Option={"Add", "Multiply", "Screen"}, Order=2)]

TintScript = {}
TintScript.__index = TintScript

function TintScript.new(construct)
    local self = setmetatable({}, TintScript)
    self.curTime = 0.0
    self.intensity = 1.0
    self.speed = 1.0
    if construct then self:constructor() end
    return self
end

function TintScript:constructor()
end

function TintScript:onStart(comp)
    self.material = comp.entity:getComponent("MeshRenderer").material
    self.width = Amaz.BuiltinObject:getInputTextureWidth()
    self.height = Amaz.BuiltinObject.getInputTextureHeight()
    Amaz.LOGI("chukcut_tint", "started")
    self.material:setFloat("u_intensity", self.intensity)
end

function TintScript:onUpdate(comp, deltaTime)
    self.curTime = self.curTime + deltaTime
    self:seekToTime(comp, self.curTime)
end

function TintScript:seekToTime(comp, time)
    self.material["u_time"] = time * self.speed
    self.material:setVec3("u_tint", Amaz.Vector3f(1.0, 0.55, 0.3))
end

function TintScript:onEvent(sys, event)
    if event.args:get(0) == "effects_adjust_intensity" then
        self.intensity = event.args:get(1)
        self.material:setFloat("u_intensity", self.intensity)
    end
end

function TintScript:onDestroy(comp)
end
