import QtQuick 2.15
import QtQuick.Window 2.15

Window {
    id: root
    flags: Qt.FramelessWindowHint | Qt.WindowTransparentForInput | Qt.WindowStaysOnTopHint | Qt.Tool
    color: "transparent"
    x: 0; y: 0
    width:  overlay.state["surface_w"] || 1920
    height: overlay.state["surface_h"] || 1080
    visible: overlay.visible

    Canvas {
        id: canvas
        anchors.fill: parent

        onPaint: {
            var ctx = getContext("2d")
            ctx.clearRect(0, 0, width, height)

            var s = overlay.state
            if (!s || s["cursor_x"] === undefined) return

            var config   = s["config"]   || {}
            var cursorX  = s["cursor_x"] || 0
            var cursorY  = s["cursor_y"] || 0
            var surfaceH = s["surface_h"] || height
            var sizePct  = ((config["spot_size_pct"] || 20) / 100.0)
            var spotR    = (surfaceH * sizePct) / 2.0
            var rotation = (config["spot_rotation_deg"] || 0) * Math.PI / 180.0
            var shape    = config["shape"] || { "type": "circle" }

            // 1. Shade layer
            if (config["show_spot_shade"] !== false) {
                ctx.globalCompositeOperation = "source-over"
                ctx.globalAlpha = config["shade_opacity"] !== undefined ? config["shade_opacity"] : 0.7
                ctx.fillStyle   = config["shade_color"] || "#000000"
                ctx.fillRect(0, 0, width, height)
            }

            // 2. Punch the transparent hole
            ctx.globalCompositeOperation = "destination-out"
            ctx.globalAlpha = 1.0
            buildShapePath(ctx, cursorX, cursorY, spotR, shape, rotation)
            ctx.fill()

            // 3. Border ring (stroke just outside the hole)
            if (config["show_border"]) {
                var borderPct   = (config["border_size_pct"] || 5) / 100.0
                var borderWidth = spotR * borderPct * 2.0
                ctx.globalCompositeOperation = "source-over"
                ctx.globalAlpha = config["border_opacity"] !== undefined ? config["border_opacity"] : 1.0
                ctx.strokeStyle = config["border_color"] || "#ffffff"
                ctx.lineWidth   = borderWidth
                buildShapePath(ctx, cursorX, cursorY, spotR + borderWidth / 2.0, shape, rotation)
                ctx.stroke()
            }

            // 4. Center dot (drawn over the transparent hole - shows through to screen)
            if (config["show_center_dot"]) {
                var dotR = (config["dot_size_px"] || 8) / 2.0
                ctx.globalCompositeOperation = "source-over"
                ctx.globalAlpha = config["dot_opacity"] !== undefined ? config["dot_opacity"] : 1.0
                ctx.fillStyle   = config["dot_color"] || "#ffffff"
                ctx.beginPath()
                ctx.arc(cursorX, cursorY, dotR, 0, 2 * Math.PI)
                ctx.fill()
            }
        }

        // Build a closed path for the given shape.  Does NOT call fill() or stroke().
        function buildShapePath(ctx, cx, cy, r, shape, rotation) {
            var type = shape["type"] || "circle"
            ctx.beginPath()
            if (type === "circle") {
                ctx.arc(cx, cy, r, 0, 2 * Math.PI)
            } else if (type === "square") {
                var cr = r * 2.0 * ((shape["corner_radius_pct"] || 0) / 100.0)
                roundedRectPath(ctx, cx - r, cy - r, r * 2.0, r * 2.0, cr, rotation)
            } else if (type === "star") {
                var innerR = r * ((shape["inner_radius_pct"] || 50) / 100.0)
                starPath(ctx, cx, cy, r, shape["points"] || 5, innerR, rotation)
            } else if (type === "ngon") {
                ngonPath(ctx, cx, cy, r, shape["sides"] || 6, rotation)
            } else {
                ctx.arc(cx, cy, r, 0, 2 * Math.PI)
            }
            ctx.closePath()
        }

        // Rounded rectangle path with optional rotation.
        function roundedRectPath(ctx, x, y, w, h, r, rotation) {
            if (rotation !== 0) {
                ctx.save()
                ctx.translate(x + w / 2, y + h / 2)
                ctx.rotate(rotation)
                ctx.translate(-(x + w / 2), -(y + h / 2))
            }
            ctx.moveTo(x + r, y)
            ctx.lineTo(x + w - r, y)
            ctx.arcTo(x + w, y,     x + w, y + r,     r)
            ctx.lineTo(x + w, y + h - r)
            ctx.arcTo(x + w, y + h, x + w - r, y + h, r)
            ctx.lineTo(x + r, y + h)
            ctx.arcTo(x,     y + h, x,     y + h - r, r)
            ctx.lineTo(x,     y + r)
            ctx.arcTo(x,     y,     x + r, y,         r)
            if (rotation !== 0) ctx.restore()
        }

        // Star path: alternates outer/inner vertices.
        function starPath(ctx, cx, cy, outerR, points, innerR, rotation) {
            var base = -Math.PI / 2 + rotation
            for (var i = 0; i < points; i++) {
                var outerAngle = base + (i * 2 * Math.PI / points)
                var innerAngle = outerAngle + Math.PI / points
                var ox = cx + outerR * Math.cos(outerAngle)
                var oy = cy + outerR * Math.sin(outerAngle)
                if (i === 0) ctx.moveTo(ox, oy)
                else         ctx.lineTo(ox, oy)
                ctx.lineTo(cx + innerR * Math.cos(innerAngle),
                           cy + innerR * Math.sin(innerAngle))
            }
        }

        // Regular n-gon path.
        function ngonPath(ctx, cx, cy, r, sides, rotation) {
            var base = -Math.PI / 2 + rotation
            for (var i = 0; i < sides; i++) {
                var a  = base + (i * 2 * Math.PI / sides)
                var px = cx + r * Math.cos(a)
                var py = cy + r * Math.sin(a)
                if (i === 0) ctx.moveTo(px, py)
                else         ctx.lineTo(px, py)
            }
        }
    }

    Connections {
        target: overlay
        function onStateChanged()      { canvas.requestPaint() }
        function onVisibilityChanged() { canvas.requestPaint() }
    }
}
