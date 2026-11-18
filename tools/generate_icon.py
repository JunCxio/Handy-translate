"""生成应用图标及标准 Windows 资源文件；需要 Pillow。"""
from pathlib import Path
import struct
from PIL import Image, ImageDraw

assets = Path(__file__).resolve().parents[1] / 'assets'
assets.mkdir(exist_ok=True)
svg = '''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">
<rect x="12" y="12" width="232" height="232" rx="52" fill="#3d61cf"/>
<path d="M54 72h56l18 16v108l-26-14H54z" fill="#fff"/>
<path d="M140 88l18-16h44v110h-36l-26 14z" fill="#c7ecfa"/>
<g fill="none" stroke="#3d61cf" stroke-width="9" stroke-linecap="round" stroke-linejoin="round">
<path d="M74 105h24m-24 24h30m-30 24h20M159 111h24m-12-11 12 11-12 11M183 153h-24m12-11-12 11 12 11"/>
</g></svg>'''
(assets / 'icon.svg').write_text(svg + '\n', encoding='utf-8')

scale = 4
image = Image.new('RGBA', (256 * scale, 256 * scale))
draw = ImageDraw.Draw(image)
def points(values):
    return [(x * scale, y * scale) for x, y in values]
draw.rounded_rectangle((12*scale, 12*scale, 244*scale, 244*scale), radius=52*scale, fill='#3d61cf')
draw.polygon(points([(54,72),(110,72),(128,88),(128,196),(102,182),(54,182)]), fill='#ffffff')
draw.polygon(points([(140,88),(158,72),(202,72),(202,182),(166,182),(140,196)]), fill='#c7ecfa')
for line in [[(74,105),(98,105)],[(74,129),(104,129)],[(74,153),(94,153)],[(159,111),(183,111)],[(171,100),(183,111),(171,122)],[(183,153),(159,153)],[(171,142),(159,153),(171,164)]]:
    draw.line(points(line), fill='#3d61cf', width=9*scale, joint='curve')
    for x,y in line:
        r=4.5
        draw.ellipse(((x-r)*scale,(y-r)*scale,(x+r)*scale,(y+r)*scale), fill='#3d61cf')
image = image.resize((256,256), Image.Resampling.LANCZOS)
image.save(assets / 'icon.png')
image.save(assets / 'icon.ico', sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])
(assets / 'icon.rgba').write_bytes(image.resize((64,64), Image.Resampling.LANCZOS).tobytes())

# .res 可由 MSVC 与 LLVM 的 Windows 链接器直接读取。
def resource(kind, name, data):
    header = struct.pack('<IIHHHHIHHII', len(data),32,0xffff,kind,0xffff,name,0,0x1030,0x409,0,0)
    return header + data + b'\0' * (-len(data) % 4)
ico = (assets / 'icon.ico').read_bytes()
reserved, kind, count = struct.unpack_from('<HHH', ico)
assert reserved == 0 and kind == 1 and count == 7
records = struct.pack('<IIHHHHIHHII',0,32,0xffff,0,0xffff,0,0,0,0,0,0)
group = ico[:6]
for index in range(count):
    width,height,colors,pad,planes,bits,length,offset = struct.unpack_from('<BBBBHHII',ico,6+16*index)
    payload = ico[offset:offset+length]
    assert len(payload) == length
    records += resource(3,index+1,payload)
    group += struct.pack('<BBBBHHIH',width,height,colors,pad,planes,bits,length,index+1)
(assets / 'icon.res').write_bytes(records + resource(14,1,group))
assert len((assets / 'icon.rgba').read_bytes()) == 64*64*4
assert Image.open(assets / 'icon.ico').ico.sizes() == {(n,n) for n in [16,24,32,48,64,128,256]}
print('已生成 SVG、PNG、7 种尺寸的 ICO 及 Windows 资源文件。')
