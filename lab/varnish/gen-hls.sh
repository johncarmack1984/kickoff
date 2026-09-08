#!/bin/bash
set -euo pipefail

mkdir -p /content

echo "[origin] generating 30-second test-pattern HLS ladder..."
ffmpeg -y -loglevel warning \
  -f lavfi -i "testsrc2=duration=30:size=640x360:rate=24" \
  -f lavfi -i "sine=frequency=440:duration=30:sample_rate=44100" \
  -c:v libx264 -preset ultrafast -g 96 -sc_threshold 0 -b:v 800k \
  -c:a aac -b:a 64k -ar 44100 \
  -f hls -hls_time 4 -hls_playlist_type vod \
  -hls_segment_type fmp4 \
  -hls_fmp4_init_filename init.mp4 \
  -hls_segment_filename '/content/seg_%03d.m4s' \
  /content/360p.m3u8

cat > /content/master.m3u8 << 'MASTER'
#EXTM3U
#EXT-X-VERSION:7
#EXT-X-STREAM-INF:BANDWIDTH=864000,RESOLUTION=640x360,CODECS="avc1.42c01e,mp4a.40.2"
360p.m3u8
MASTER

echo "[origin] HLS content ready, serving on :80"
cd /content
exec python3 -m http.server 80
