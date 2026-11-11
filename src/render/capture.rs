//! Blocking readback is deliberately exclusive to offline export. Native play
//! keeps the asynchronous screenshot path and its normal in-flight limit.
use super::*;

pub(crate) struct Readback { buffer: wgpu::Buffer, row_bytes: u32, width: u32, height: u32 }
impl Renderer {
    pub(crate) fn replay_readback(&self) -> Result<Readback,String> {
        if self.surface.is_some() { return Err("replay readback requires an offscreen renderer".into()); }
        let width=self.config.width;
        let height=self.config.height;
        let row_bytes=(width*4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)*wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer=self.device.create_buffer(&wgpu::BufferDescriptor {
            label:Some("replay reusable RGBA readback"),size:u64::from(row_bytes)*u64::from(height),
            usage:wgpu::BufferUsages::COPY_DST|wgpu::BufferUsages::MAP_READ,mapped_at_creation:false,
        });
        Ok(Readback {buffer,row_bytes,width,height})
    }
    pub(crate) fn replay_rgba(&mut self, scene:&Scene, observation:&Observation, readback:&Readback, pixels:&mut Vec<u8>) -> Result<(),String> {
        if self.surface.is_some() || (self.config.width,self.config.height)!=(readback.width,readback.height) {
            return Err("replay readback target changed".into());
        }
        let previous=self.frame_id;
        self.render(scene,None,observation)?;
        if self.frame_id!=previous+1 { return Err("offscreen replay frame was not submitted".into()); }
        let mut encoder=self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {label:Some("replay copy")});
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {texture:&self.targets.color,mip_level:0,origin:wgpu::Origin3d::ZERO,aspect:wgpu::TextureAspect::All},
            wgpu::TexelCopyBufferInfo {buffer:&readback.buffer,layout:wgpu::TexelCopyBufferLayout {offset:0,bytes_per_row:Some(readback.row_bytes),rows_per_image:Some(readback.height)}},
            wgpu::Extent3d {width:readback.width,height:readback.height,depth_or_array_layers:1});
        let submission=self.queue.submit([encoder.finish()]);
        let (send,receive)=mpsc::sync_channel(1);
        readback.buffer.slice(..).map_async(wgpu::MapMode::Read,move |result| {let _=send.send(result);});
        self.device.poll(wgpu::PollType::Wait {submission_index:Some(submission),timeout:Some(std::time::Duration::from_secs(120))})
            .map_err(|e| format!("replay GPU completion: {e}"))?;
        receive.recv().map_err(|e| e.to_string())?.map_err(|e| format!("replay map RGBA: {e}"))?;
        let mapped=readback.buffer.slice(..).get_mapped_range();
        pixels.clear();
        pixels.reserve((readback.width*readback.height*4) as usize);
        for row in mapped.chunks_exact(readback.row_bytes as usize) {
            pixels.extend_from_slice(&row[..readback.width as usize*4]);
        }
        drop(mapped);
        readback.buffer.unmap();
        if let Ok(error)=self.gpu_errors.try_recv() { return Err(error); }
        Ok(())
    }
}
