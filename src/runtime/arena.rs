// arena.rs — bump allocator for Cx function scopes

const DEFAULT_CHUNK_SIZE: usize = 65536; // 64KB

pub struct Chunk {
    data: Vec<u8>,
    offset: usize,
}

impl Chunk {
    pub fn new(size: usize) -> Self {
        Self {
            data: vec![0u8; size],
            offset: 0,
        }
    }

    pub fn alloc(&mut self, size: usize, align: usize) -> Option<*mut u8> {
        // calculate aligned offset
        let aligned = (self.offset + align - 1) & !(align - 1);
        if aligned + size > self.data.len() {
            return None; // chunk exhausted
        }
        let ptr = unsafe { self.data.as_mut_ptr().add(aligned) };
        self.offset = aligned + size;
        Some(ptr)
    }

    pub fn reset(&mut self) {
        self.offset = 0;
    }
}

pub struct Arena {
    chunks: Vec<Chunk>,
    current: usize, // index of active chunk
}

impl Arena {
    /// Creates an arena with NO backing chunk (tracker #010). The 64 KB chunk
    /// is allocated lazily on the first `alloc()`, so a function scope that
    /// never allocates into the arena — pure arithmetic, recursion without heap
    /// locals, the common call-heavy case — pays nothing. Previously every
    /// `push_function_scope` eagerly built and zero-filled a 64 KB chunk,
    /// measured by Pillar 1 at ~90% of call-heavy workload instructions.
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            current: 0,
        }
    }

    pub fn alloc(&mut self, size: usize, align: usize) -> *mut u8 {
        // Lazily allocate the first chunk on demand (see `new`).
        if self.chunks.is_empty() {
            let new_size = DEFAULT_CHUNK_SIZE.max(size + align);
            self.chunks.push(Chunk::new(new_size));
            self.current = 0;
        }

        // try current chunk first
        if let Some(ptr) = self.chunks[self.current].alloc(size, align) {
            return ptr;
        }

        // current chunk exhausted — chain a new one
        // new chunk is at least as big as requested or default size
        let new_size = DEFAULT_CHUNK_SIZE.max(size + align);
        self.chunks.push(Chunk::new(new_size));
        self.current += 1;

        // guaranteed to succeed on fresh chunk
        self.chunks[self.current]
            .alloc(size, align)
            .expect("fresh chunk failed to allocate — this is a bug")
    }

    pub fn alloc_str(&mut self, s: &str) -> &str {
        let bytes = s.as_bytes();
        let ptr = self.alloc(bytes.len() + 1, 1);
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            *ptr.add(bytes.len()) = 0; // null terminator
            let slice = std::slice::from_raw_parts(ptr, bytes.len());
            std::str::from_utf8_unchecked(slice)
        }
    }

    pub fn reset(&mut self) {
        // reset all chunks, drop extras, keep only first (if any chunk exists —
        // a lazily-created arena may hold none; #010)
        self.chunks.truncate(1);
        if let Some(first) = self.chunks.first_mut() {
            first.reset();
        }
        self.current = 0;
    }

    pub fn bytes_used(&self) -> usize {
        self.chunks.iter().map(|c| c.offset).sum()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
}

impl Default for Arena {
    fn default() -> Self {
        Self::new()
    }
}
