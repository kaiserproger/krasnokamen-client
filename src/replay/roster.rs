//! Exact UUID rosters and one-shot Linux publication notifications.
//! A dynamic roster is published after recorder join by close-write or atomic
//! rename. We watch its existing parent before join; there is no file polling.
use std::{collections::BTreeSet, fs::File, io::Read, path::{Path,PathBuf}, sync::{Arc,mpsc}};

type Result<T, E=String> = std::result::Result<T,E>;
pub(crate) type Roster = BTreeSet<String>;
pub(crate) fn load(path:&Path) -> Result<Roster> {
    let mut text=String::new();
    File::open(path).map_err(|e|format!("open recorder roster {}: {e}",path.display()))?
        .take(4*1024*1024+1).read_to_string(&mut text).map_err(|e|format!("read roster {}: {e}",path.display()))?;
    if text.len()>4*1024*1024 {return Err("recorder UUID roster exceeds 4 MiB".into());}
    let mut roster=Roster::new();
    for line in text.lines().map(str::trim).filter(|line|!line.is_empty()) {
        let uuid=uuid::Uuid::parse_str(line).map_err(|e|format!("recorder roster UUID {line}: {e}"))?.to_string();
        if !roster.insert(uuid) {return Err("recorder UUID roster has duplicate entries".into());}
    }
    if roster.is_empty() || roster.len()>100_000 {return Err("recorder UUID roster requires 1..100000 unique UUIDs".into());}
    Ok(roster)
}

pub(crate) struct Watch {
    result:mpsc::Receiver<Result<Roster>>,
    #[cfg(target_os="linux")]
    cancel:std::os::unix::net::UnixStream,
    #[cfg(target_os="linux")]
    worker:Option<std::thread::JoinHandle<()>>,
}
impl Watch {
    #[cfg(target_os="linux")]
    pub(crate) fn start(path:PathBuf,wake:Arc<dyn Fn()+Send+Sync>) -> Result<Self> {
        use std::{ffi::CString,os::{fd::{AsRawFd,FromRawFd,OwnedFd},unix::{ffi::OsStrExt,net::UnixStream}},thread};
        let parent=path.parent().filter(|p|!p.as_os_str().is_empty()).unwrap_or_else(||Path::new("."));
        let parent_name=CString::new(parent.as_os_str().as_bytes()).map_err(|_|"roster directory contains NUL")?;
        let filename=path.file_name().ok_or("roster path has no filename")?.as_bytes().to_vec();
        // SAFETY: no pointers; a successful descriptor is owned exactly once.
        let descriptor=unsafe {libc::inotify_init1(libc::IN_CLOEXEC)};
        if descriptor<0 {return Err(format!("watch roster {}: {}",path.display(),std::io::Error::last_os_error()));}
        let notification=unsafe {OwnedFd::from_raw_fd(descriptor)};
        let mask=libc::IN_CLOSE_WRITE|libc::IN_MOVED_TO|libc::IN_DELETE_SELF|libc::IN_MOVE_SELF|libc::IN_ONLYDIR;
        // SAFETY: valid owned descriptor and NUL-terminated parent path.
        if unsafe {libc::inotify_add_watch(notification.as_raw_fd(),parent_name.as_ptr(),mask)}<0 {
            return Err(format!("watch roster directory {}: {}",parent.display(),std::io::Error::last_os_error()));
        }
        if path.try_exists().map_err(|e|e.to_string())? {
            return Err(format!("dynamic roster {} already exists; use a fresh path or --ready-entity-uuids",path.display()));
        }
        let (cancel,stop)=UnixStream::pair().map_err(|e|e.to_string())?;
        let (send,result)=mpsc::sync_channel(1);
        let worker=thread::Builder::new().name("recorder-roster".into()).spawn(move || {
            let loaded=linux_wait(&notification,&stop,&filename).and_then(|published| {
                if published {load(&path).map(Some)} else {Ok(None)}
            });
            match loaded {
                Ok(None)=>{}, // Drop cancelled the one-shot watcher.
                Ok(Some(roster))=>{let _=send.send(Ok(roster));wake();},
                Err(error)=>{let _=send.send(Err(error));wake();},
            }
        }).map_err(|e|format!("start roster watcher: {e}"))?;
        Ok(Self {result,cancel,worker:Some(worker)})
    }
    #[cfg(not(target_os="linux"))]
    pub(crate) fn start(_path:PathBuf,_wake:Arc<dyn Fn()+Send+Sync>) -> Result<Self> {
        Err("post-join roster publication notifications require Linux; static --ready-entity-uuids is supported".into())
    }
    pub(crate) fn take_result(&self) -> Result<Option<Roster>> {
        match self.result.try_recv() {
            Ok(result)=>result.map(Some),
            Err(mpsc::TryRecvError::Empty)=>Ok(None),
            Err(mpsc::TryRecvError::Disconnected)=>Err("roster watcher ended without a publication result".into()),
        }
    }
}
#[cfg(target_os="linux")]
impl Drop for Watch {
    fn drop(&mut self) {
        let _=self.cancel.shutdown(std::net::Shutdown::Both);
        if let Some(worker)=self.worker.take() {let _=worker.join();}
    }
}

#[cfg(target_os="linux")]
fn linux_wait(notification:&std::os::fd::OwnedFd,stop:&std::os::unix::net::UnixStream,filename:&[u8]) -> Result<bool> {
    use std::{io,os::fd::AsRawFd};
    let mut descriptors=[
        libc::pollfd {fd:notification.as_raw_fd(),events:libc::POLLIN,revents:0},
        libc::pollfd {fd:stop.as_raw_fd(),events:libc::POLLIN,revents:0},
    ];
    let mut bytes=[0u8;4096];
    loop {
        // SAFETY: two initialized poll descriptors; -1 blocks on events, not a timer.
        let ready=unsafe {libc::poll(descriptors.as_mut_ptr(),2,-1)};
        if ready<0 {
            let error=io::Error::last_os_error();
            if error.kind()==io::ErrorKind::Interrupted {continue;}
            return Err(format!("wait for roster publication: {error}"));
        }
        if descriptors[1].revents!=0 {return Ok(false);}
        if descriptors[0].revents & (libc::POLLERR|libc::POLLHUP|libc::POLLNVAL)!=0 {
            return Err("roster notification descriptor failed".into());
        }
        // SAFETY: descriptor is readable and bytes points to a writable buffer.
        let read=unsafe {libc::read(notification.as_raw_fd(),bytes.as_mut_ptr().cast(),bytes.len())};
        if read<0 {
            let error=io::Error::last_os_error();
            if error.kind()==io::ErrorKind::Interrupted {continue;}
            return Err(format!("read roster notification: {error}"));
        }
        if read==0 {return Err("roster notification stream closed".into());}
        let mut offset=0;
        let header=std::mem::size_of::<libc::inotify_event>();
        while offset+header<=read as usize {
            // SAFETY: header bounds checked; read_unaligned makes byte-buffer alignment irrelevant.
            let event=unsafe {std::ptr::read_unaligned(bytes[offset..].as_ptr().cast::<libc::inotify_event>())};
            let end=offset+header+event.len as usize;
            if end>read as usize {return Err("truncated roster notification".into());}
            if event.mask & (libc::IN_Q_OVERFLOW|libc::IN_IGNORED|libc::IN_DELETE_SELF|libc::IN_MOVE_SELF)!=0 {
                return Err("roster directory watch lost or overflowed".into());
            }
            let name=&bytes[offset+header..end];
            let name=&name[..name.iter().position(|byte|*byte==0).unwrap_or(name.len())];
            if name==filename && event.mask & (libc::IN_CLOSE_WRITE|libc::IN_MOVED_TO)!=0 {return Ok(true);}
            offset=end;
        }
    }
}

#[cfg(all(test,target_os="linux"))]
mod tests {
    use super::*;
    use std::{io::Write,time::{Duration,SystemTime,UNIX_EPOCH}};
    fn directory() -> PathBuf {
        let path=std::env::temp_dir().join(format!("roster-watch-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&path).unwrap();path
    }
    #[test]
    fn publication_after_join_uses_close_write_or_atomic_rename() {
        for rename in [false,true] {
            let directory=directory();
            let path=directory.join("roster.txt");
            let (send,wake)=mpsc::sync_channel(1);
            let watch=Watch::start(path.clone(),Arc::new(move ||{let _=send.send(());})).unwrap();
            let temporary=if rename {directory.join("staging.tmp")} else {path.clone()};
            let mut file=File::create(&temporary).unwrap();
            writeln!(file,"00000000-0000-0000-0000-000000000001").unwrap();
            // Creating/writing without close is not a completed publication.
            assert!(matches!(wake.try_recv(),Err(mpsc::TryRecvError::Empty)));
            drop(file);
            if rename {std::fs::rename(temporary,&path).unwrap();}
            wake.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(watch.take_result().unwrap().unwrap(),Roster::from(["00000000-0000-0000-0000-000000000001".into()]));
            drop(watch);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
    #[test]
    fn unpublished_watch_cancels_and_invalid_roster_fails() {
        let directory=directory();
        let path=directory.join("roster.txt");
        let watch=Watch::start(path.clone(),Arc::new(||{})).unwrap();
        drop(watch); // Cancellation wakes the blocked worker; no timer or file event required.
        std::fs::write(&path,"not-a-uuid\n").unwrap();
        assert!(load(&path).is_err());
        assert!(Watch::start(path.clone(),Arc::new(||{})).is_err(),"stale dynamic roster must fail closed");
        std::fs::write(&path,"00000000-0000-0000-0000-000000000001\n00000000000000000000000000000001\n").unwrap();
        assert!(load(&path).unwrap_err().contains("duplicate"));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
