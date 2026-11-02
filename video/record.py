"""Record the testnet replay page to video/demo.mp4 (+ demo.gif).

  python video/record.py --rust 41 --ts 16 --verify "payments 7: released 4, refunded 3 ... verify: OK"

Data comes from deployments/testnet.json (written by the deploy and demo scripts); nothing is typed by hand.
Needs Python Playwright (chromium) and ffmpeg on PATH.
"""
import argparse, json, pathlib, subprocess
from playwright.sync_api import sync_playwright

ROOT = pathlib.Path(__file__).resolve().parent.parent
a = argparse.ArgumentParser()
a.add_argument("--rust", type=int, required=True)
a.add_argument("--ts", type=int, required=True)
a.add_argument("--verify", required=True)
a.add_argument("--data", default=str(ROOT / "deployments/testnet.json"))
args = a.parse_args()

data = json.loads(pathlib.Path(args.data).read_text(encoding="utf-8"))
data["tests"] = {"rust": args.rust, "ts": args.ts}
data["verify"] = args.verify
out = ROOT / "video"
with sync_playwright() as p:
    b = p.chromium.launch()
    ctx = b.new_context(viewport={"width": 1280, "height": 720}, record_video_dir=str(out / "_raw"),
                        record_video_size={"width": 1280, "height": 720})
    page = ctx.new_page()
    page.add_init_script("window.DATA = " + json.dumps(data) + ";")
    page.goto((out / "replay.html").as_uri())
    page.wait_for_function("window.DONE === true", timeout=150_000)
    raw = page.video.path()
    ctx.close()
    b.close()
subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", raw, "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "26",
                "-movflags", "+faststart", str(out / "demo.mp4")], check=True)
subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", str(out / "demo.mp4"), "-vf",
                "fps=6,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=64[p];[b][p]paletteuse",
                str(out / "demo.gif")], check=True)
print("video/demo.mp4, video/demo.gif")
