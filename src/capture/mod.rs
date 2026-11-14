//! Fixed 60 Hz world-only export, two-times-per-axis SSAA, optional 180-degree
//! shutter accumulation. Encoded-RGBA averaging is explicit (not linear-HDR blur).
pub mod camera;
#[cfg(test)]
mod gpu_tests;
use crate::{content_paths, render::{GraphicsPreset, Renderer}, replay::{Player, clock::frame_time}};
use sha2::{Digest, Sha256};
use std::{fs::{File, OpenOptions}, io::{BufWriter, Read, Write}, path::{Path, PathBuf}, process::{Child, ChildStdin, Command, Stdio}, time::Duration};

struct Encoder { child: Child, input: Option<ChildStdin>, completed: bool }
impl Encoder {
    fn start(binary:&str, width:u32, height:u32, path:&Path) -> Result<Self,String> {
        let mut child=Command::new(binary).args(["-nostdin","-n","-hide_banner","-loglevel","warning","-f","rawvideo","-pix_fmt","rgba","-video_size"])
            .arg(format!("{width}x{height}")).args(["-framerate","60","-i","pipe:0","-an","-c:v","libx264","-preset","slow","-crf","18","-pix_fmt","yuv420p","-color_primaries","bt709","-color_trc","bt709","-colorspace","bt709","-movflags","+faststart"])
            .arg(path).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().map_err(|e| format!("spawn {binary}: {e}"))?;
        let input=child.stdin.take();
        Ok(Self {child,input,completed:false})
    }
    fn frame(&mut self,pixels:&[u8])->Result<(),String> {
        self.input.as_mut().ok_or("ffmpeg input closed")?.write_all(pixels).map_err(|e| format!("pipe frame to ffmpeg: {e}"))
    }
    fn finish(mut self)->Result<(),String> {
        self.input.take();
        let status=self.child.wait().map_err(|e| e.to_string())?;
        self.completed=true;
        if !status.success() {return Err(format!("ffmpeg exited {status}"));}
        Ok(())
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        if !self.completed {self.input.take();let _=self.child.kill();let _=self.child.wait();}
    }
}

/// Accumulate one 2x-per-axis SSAA image into an output-sized shutter sum.
fn accumulate(rgba:&[u8],width:usize,height:usize,sum:&mut [u32]) {
    for y in 0..height { for x in 0..width { for channel in 0..4 {
        let i=((2*y)*(2*width)+2*x)*4+channel;
        sum[(y*width+x)*4+channel] += u32::from(rgba[i])+u32::from(rgba[i+4])
            +u32::from(rgba[i+2*width*4])+u32::from(rgba[i+2*width*4+4]);
    }}}
}

pub(crate) fn cli(mut args:impl Iterator<Item=String>)->Result<(),String> {
    let (mut recording,mut camera,mut output,mut data)=(None,None,None,None);
    let (mut start,mut end)=(None,None);
    let (mut width,mut height,mut samples)=(1920u32,1080u32,1u32);
    let mut ffmpeg="ffmpeg".to_string();
    while let Some(arg)=args.next() {
        if arg=="--help" {crate::launch::print_help();return Ok(());}
        let value=args.next().ok_or_else(||format!("render-replay missing value for {arg}"))?;
        match arg.as_str() {
            "--recording"=>recording=Some(PathBuf::from(value)),"--camera"=>camera=Some(PathBuf::from(value)),
            "--output"=>output=Some(PathBuf::from(value)),"--data-dir"=>data=Some(PathBuf::from(value)),
            "--start"=>start=Some(value.parse::<f64>().map_err(|e|e.to_string())?),
            "--end"=>end=Some(value.parse::<f64>().map_err(|e|e.to_string())?),
            "--motion-samples"=>samples=value.parse::<u32>().map_err(|e|e.to_string())?,
            "--size"=>{let (w,h)=value.split_once('x').ok_or("size must be WIDTHxHEIGHT")?;width=w.parse::<u32>().map_err(|e|e.to_string())?;height=h.parse::<u32>().map_err(|e|e.to_string())?;},
            "--ffmpeg"=>ffmpeg=value,
            _=>return Err(format!("unknown render-replay argument {arg}")),
        }
    }
    if width==0 || height==0 || width>3840 || height>2160 || width%2!=0 || height%2!=0 || !(1..=16).contains(&samples) {
        return Err("render size must be even, positive, at most 3840x2160; motion-samples must be 1..16".into());
    }
    let recording=recording.ok_or("render-replay requires --recording")?;
    let camera=camera.ok_or("render-replay requires --camera")?;
    let output=output.ok_or("render-replay requires --output")?;
    if output.extension().is_none_or(|extension|extension!="mp4") || output.exists() {return Err("render output must be a new .mp4 path".into());}
    let mut bytes=Vec::new();
    File::open(&camera).map_err(|e|format!("open {}: {e}",camera.display()))?.take(1024*1024+1).read_to_end(&mut bytes).map_err(|e|e.to_string())?;
    let path=camera::CameraPath::parse(&bytes)?;
    let range=path.range();
    let start=start.unwrap_or(range[0]);
    let end=end.unwrap_or(range[1]);
    if !start.is_finite() || !end.is_finite() || start<range[0] || end>range[1] || start>=end || end>u32::MAX as f64/1000. {
        return Err("render range must be finite, increasing and within camera keys and solrec time range".into());
    }
    if end-start < 1./60. { return Err("render range must contain at least one full 1/60s frame".into()); }
    let root=data.map(Ok).unwrap_or_else(content_paths::default_root)?;
    content_paths::apply_defaults(&root);
    content_paths::validate(true)?;
    let artifacts=output.parent().filter(|p|!p.as_os_str().is_empty()).unwrap_or_else(||Path::new("."));
    let mut replay=Player::open(&recording,true)?;
    let mut renderer=pollster::block_on(Renderer::new_offscreen(width*2,height*2,artifacts.into()))?;
    replay.set_crowd_instancing(renderer.crowd_supported());
    // Medium is the non-temporal HDR/shadows preset: TAA and its projection
    // jitter/history are off by construction, regardless of persisted settings.
    renderer.set_graphics_preset(GraphicsPreset::Medium);
    let readback=renderer.replay_readback()?;
    let mut pixels=Vec::new();
    let mut sum=vec![0u32;(width*height*4) as usize];
    let mut frame=vec![0u8;sum.len()];
    let metadata_path=output.with_extension("frames.jsonl");
    let mut metadata=BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&metadata_path).map_err(|e|format!("create {}: {e}",metadata_path.display()))?);
    let mut encoder=Encoder::start(&ffmpeg,width,height,&output)?;
    let begin=Duration::from_secs_f64(start);
    let end=Duration::from_secs_f64(end);
    let mut index=0;
    while begin+frame_time(index+1,60)<=end {
        let base=begin+frame_time(index,60);
        sum.fill(0);
        let mut sample_ticks=[None;16];
        for sample in 0..samples {
            // 180 degree shutter: sample only the first half of this frame.
            let time=base+frame_time(u64::from(sample),120*samples);
            replay.advance_to(time)?;
            if replay.last_tick.is_none() { return Err("replay has no solaris:tick marker at shot start; configure recorder account".into()); }
            let partial=replay.partial();
            replay.session.replay_camera(&path,time.as_secs_f64(),partial)?;
            replay.session.prepare_frame(partial)?;
            replay.session.settle_replay_geometry()?;
            if replay.session.phase()!="play" {return Err(format!("render starts before Play at {:.3}s",time.as_secs_f64()));}
            let observation=replay.session.execution_observation();
            renderer.replay_rgba(replay.session.scene(),&observation,&readback,&mut pixels)?;
            replay.session.release_uploaded_terrain(renderer.drain_uploaded_meshes());
            replay.session.far_generated(renderer.far_gpu_generation(),renderer.drain_far_generated());
            accumulate(&pixels,width as usize,height as usize,&mut sum);
            sample_ticks[sample as usize]=replay.last_tick;
        }
        let divisor=4*samples;
        for (pixel,sum) in frame.iter_mut().zip(&sum) {*pixel=((sum+divisor/2)/divisor) as u8;}
        encoder.frame(&frame)?;
        writeln!(metadata,"{}",serde_json::json!({"frame":index,"replay_time":base.as_secs_f64(),
            "last_applied_tick":replay.last_tick,"wall_ns":replay.last_wall_ns,"sample_ticks":&sample_ticks[..samples as usize],
            "camera":replay.session.replay_pose(),"cpu_state_sha256":replay.frame_hash(),
            "rgba_sha256":format!("{:x}",Sha256::digest(&frame)),"supersampling":2,"motion_samples":samples,"taa":false}))
            .map_err(|e|format!("write replay frame metadata: {e}"))?;
        index+=1;
    }
    encoder.finish()?;
    metadata.flush().map_err(|e|format!("flush frame metadata: {e}"))?;
    eprintln!("rendered {index} frames: {} (metadata {})",output.display(),metadata_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supersampling_and_shutter_average_all_pixels_not_padding() {
        let mut sum=vec![0;8];
        let input:Vec<u8>=(0..32).collect();
        accumulate(&input,2,1,&mut sum);
        assert_eq!(sum,vec![40,44,48,52,72,76,80,84]);
        accumulate(&input,2,1,&mut sum);
        assert_eq!(sum,vec![80,88,96,104,144,152,160,168]);
    }
}
