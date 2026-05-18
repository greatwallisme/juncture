# 异步I/O操作

## 当使用此专题

- 实现高性能异步I/O
- 构建非阻塞文件操作
- 理解异步事件循环
- 优化I/O密集型应用

## 核心功能

### 异步I/O分析
```
Analyze async I/O: target=src/, detect_blocking_calls=true, analyze_event_loop=true, suggest_async_alternatives=true
```
分析异步I/O实现

### 性能优化建议
```
Optimize I/O performance: code=src/, buffer_size=8192, aio=true, zero_copy=true
```
优化I/O性能

## 📚 异步文件操作基础

### Tokio异步文件操作
```rust
use tokio::fs::{File, OpenOptions};
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use std::path::Path;

/// 异步读取文件
async fn read_file_async<P: AsRef<Path>>(path: P) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = File::open(path).await?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).await?;
    Ok(contents)
}

/// 异步写入文件
async fn write_file_async<P: AsRef<Path>>(path: P, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .await?;

    file.write_all(content.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

/// 异步文件复制
async fn copy_file_async<P: AsRef<Path>>(src: P, dst: P) -> Result<u64, Box<dyn std::error::Error>> {
    let mut src_file = File::open(src).await?;
    let mut dst_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dst)
        .await?;

    io::copy(&mut src_file, &mut dst_file).await
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 写入测试文件
    let content = "Hello, async file operations!\nThis is a test file.\n";
    write_file_async("test.txt", content).await?;
    println!("File written successfully");

    // 读取文件
    let read_content = read_file_async("test.txt").await?;
    println!("File content: {}", read_content);

    // 复制文件
    let bytes_copied = copy_file_async("test.txt", "test_copy.txt").await?;
    println!("Copied {} bytes", bytes_copied);

    Ok(())
}
```

### 缓冲异步I/O
```rust
use tokio::io::{AsyncBufReadExt, BufReader, AsyncWriteExt, BufWriter};
use tokio::fs::File;

/// 缓冲读取大文件
async fn buffered_read<P: AsRef<Path>>(path: P) -> Result<usize, Box<dyn std::error::Error>> {
    let file = File::open(path).await?;
    let mut reader = BufReader::new(file);
    let mut line_count = 0;
    let mut line = String::new();

    while reader.read_line(&mut line).await? > 0 {
        line_count += 1;
        line.clear(); // 重置行缓冲区
    }

    Ok(line_count)
}

/// 缓冲写入
async fn buffered_write<P: AsRef<Path>>(path: P) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path).await?;
    let mut writer = BufWriter::new(file);

    for i in 0..1000 {
        writeln!(writer, "Line {}", i).await?;
    }

    writer.flush().await?;
    Ok(())
}

/// 流式处理大文件
async fn process_large_file<P: AsRef<Path>>(path: P) -> Result<usize, Box<dyn std::error::Error>> {
    let file = File::open(path).await?;
    let mut reader = BufReader::with_capacity(8192, file); // 8KB缓冲区

    let mut word_count = 0;
    let mut buffer = String::new();

    while reader.read_line(&mut buffer).await? > 0 {
        word_count += buffer.split_whitespace().count();
        buffer.clear();
    }

    Ok(word_count)
}
```

## 自定义异步I/O

### 异步文件监控
```rust
use tokio::fs;
use std::path::Path;
use std::time::Duration;

/// 异步文件监控器
struct FileWatcher {
    watched_files: Vec<String>,
}

impl FileWatcher {
    fn new() -> Self {
        Self {
            watched_files: Vec::new(),
        }
    }

    fn add_file<P: AsRef<Path>>(&mut self, path: P) {
        self.watched_files.push(path.as_ref().to_string_lossy().to_string());
    }

    async fn watch_changes(&self) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            for file_path in &self.watched_files {
                let metadata = fs::metadata(file_path).await?;
                let modified = metadata.modified()?;

                println!("File {} last modified: {:?}", file_path, modified);
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

/// 异步目录扫描
async fn scan_directory<P: AsRef<Path>>(dir_path: P) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut entries = fs::read_dir(dir_path).await?;
    let mut files = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file() {
            if let Some(path_str) = path.to_str() {
                files.push(path_str.to_string());
            }
        }
    }

    Ok(files)
}

/// 递归异步目录扫描
async fn scan_directory_recursive<P: AsRef<Path>>(
    dir_path: P,
    max_depth: usize,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    scan_directory_recursive_helper(dir_path, &mut files, 0, max_depth).await?;
    Ok(files)
}

async fn scan_directory_recursive_helper<P: AsRef<Path>>(
    dir_path: P,
    files: &mut Vec<String>,
    current_depth: usize,
    max_depth: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if current_depth > max_depth {
        return Ok(());
    }

    let mut entries = fs::read_dir(dir_path).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();

        if path.is_file() {
            if let Some(path_str) = path.to_str() {
                files.push(path_str.to_string());
            }
        } else if path.is_dir() {
            scan_directory_recursive_helper(&path, files, current_depth + 1, max_depth).await?;
        }
    }

    Ok(())
}
```

### 异步文件管道
```rust
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// 异步文件处理管道
struct AsyncFileProcessor {
    input_channel: mpsc::Sender<ProcessingTask>,
}

#[derive(Debug)]
struct ProcessingTask {
    input_file: String,
    output_file: String,
    operation: Operation,
}

#[derive(Debug)]
enum Operation {
    UpperCase,
    LowerCase,
    Reverse,
    CountWords,
}

impl AsyncFileProcessor {
    fn new() -> (Self, mpsc::Receiver<ProcessingTask>) {
        let (sender, receiver) = mpsc::channel(100);

        let processor = Self {
            input_channel: sender,
        };

        // 启动工作线程
        tokio::spawn(async move {
            while let Some(task) = receiver.recv().await {
                Self::process_task(task).await;
            }
        });

        (processor, receiver)
    }

    fn submit_task(&self, task: ProcessingTask) -> Result<(), mpsc::error::SendError<ProcessingTask>> {
        self.input_channel.send(task)
    }

    async fn process_task(task: ProcessingTask) {
        println!("Processing task: {:?}", task);

        // 读取输入文件
        let content = match fs::read_to_string(&task.input_file).await {
            Ok(content) => content,
            Err(e) => {
                eprintln!("Failed to read input file: {}", e);
                return;
            }
        };

        // 处理内容
        let processed_content = match task.operation {
            Operation::UpperCase => content.to_uppercase(),
            Operation::LowerCase => content.to_lowercase(),
            Operation::Reverse => content.chars().rev().collect(),
            Operation::CountWords => {
                let word_count = content.split_whitespace().count();
                word_count.to_string()
            }
        };

        // 写入输出文件
        if let Err(e) = fs::write(&task.output_file, processed_content).await {
            eprintln!("Failed to write output file: {}", e);
        } else {
            println!("Task completed: {} -> {}", task.input_file, task.output_file);
        }
    }
}

/// 文件批处理示例
async fn batch_file_processing() -> Result<(), Box<dyn std::error::Error>> {
    let (processor, _receiver) = AsyncFileProcessor::new();

    // 创建测试文件
    for i in 1..=5 {
        let content = format!("This is test file number {}", i);
        fs::write(format!("input_{}.txt", i), content).await?;
    }

    // 提交处理任务
    for i in 1..=5 {
        let task = ProcessingTask {
            input_file: format!("input_{}.txt", i),
            output_file: format!("output_{}.txt", i),
            operation: Operation::UpperCase,
        };

        if let Err(e) = processor.submit_task(task) {
            eprintln!("Failed to submit task: {}", e);
        }
    }

    // 等待处理完成
    tokio::time::sleep(Duration::from_secs(2)).await;

    Ok(())
}
```

## 高性能异步I/O

### 零拷贝操作
```rust
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use std::io::SeekFrom;
use std::pin::Pin;

/// 零拷贝文件传输
async fn zero_copy_transfer<P: AsRef<Path>>(src: P, dst: P) -> Result<u64, Box<dyn std::error::Error>> {
    let mut src_file = File::open(src).await?;
    let mut dst_file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dst)
        .await?;

    // 获取文件大小
    let file_size = src_file.metadata().await?.len();

    // 使用splice系统调用（Linux特有）
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        let mut bytes_transferred = 0u64;

        // 这里简化处理，实际应该使用nix::syscall::splice
        while bytes_transferred < file_size {
            let mut buffer = vec![0u8; 8192];
            let bytes_read = src_file.read(&mut buffer).await?;
            if bytes_read == 0 {
                break;
            }

            dst_file.write_all(&buffer[..bytes_read]).await?;
            bytes_transferred += bytes_read as u64;
        }

        Ok(bytes_transferred)
    }

    #[cfg(not(target_os = "linux"))]
    {
        // 非Linux平台使用普通拷贝
        tokio::io::copy(&mut src_file, &mut dst_file).await
    }
}

/// 内存映射文件操作
#[cfg(target_os = "linux")]
async fn memory_mapped_file_operations<P: AsRef<Path>>(path: P) -> Result<(), Box<dyn std::error::Error>> {
    use memmap2::{MmapOptions, MmapMut};

    let file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
        .await?;

    // 创建内存映射
    let mut mmap = unsafe { MmapOptions::new().map_mut(&file.into_std().await)? };

    // 直接操作内存
    mmap[0..12].copy_from_slice(b"Hello World!");
    mmap.flush()?;

    println!("Memory mapped file created successfully");
    Ok(())
}
```

### 异步I/O池
```rust
use tokio::sync::Semaphore;
use std::sync::Arc;

/// 异步I/O操作池
struct AsyncIOPool {
    semaphore: Arc<Semaphore>,
    max_concurrent_operations: usize,
}

impl AsyncIOPool {
    fn new(max_concurrent_operations: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent_operations)),
            max_concurrent_operations,
        }
    }

    async fn execute<F, R>(&self, operation: F) -> R
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        // 获取信号量许可
        let _permit = self.semaphore.acquire().await.unwrap();

        // 执行操作
        operation()
    }

    async fn read_file_with_limit<P: AsRef<Path>>(&self, path: P) -> Result<String, Box<dyn std::error::Error>> {
        let path = path.as_ref().to_string();
        self.execute(move || {
            tokio::task::block_in_place(|| {
                std::fs::read_to_string(&path)
            })
        }).await
    }

    async fn write_file_with_limit<P: AsRef<Path>>(&self, path: P, content: String) -> Result<(), Box<dyn std::error::Error>> {
        let path = path.as_ref().to_string();
        self.execute(move || {
            tokio::task::block_in_place(|| {
                std::fs::write(&path, content)
            })
        }).await
    }
}

/// 并发文件处理示例
async fn concurrent_file_processing() -> Result<(), Box<dyn std::error::Error>> {
    let io_pool = AsyncIOPool::new(5); // 最多5个并发操作

    // 创建测试文件
    for i in 1..=10 {
        let content = format!("Content of file {}", i);
        let filename = format!("file_{}.txt", i);
        io_pool.write_file_with_limit(&filename, content).await?;
        println!("Created {}", filename);
    }

    // 并发读取所有文件
    let mut handles = Vec::new();
    for i in 1..=10 {
        let filename = format!("file_{}.txt", i);
        let handle = tokio::spawn(async move {
            let content = io_pool.read_file_with_limit(&filename).await?;
            println!("Read {} successfully", filename);
            Ok::<(), Box<dyn std::error::Error>>(())
        });
        handles.push(handle);
    }

    // 等待所有读取完成
    for handle in handles {
        handle.await?;
    }

    Ok(())
}
```

## 异步I/O模式

### 流式数据处理
```rust
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::fs::File;

/// 流式处理日志文件
async fn process_log_file<P: AsRef<Path>>(path: P) -> Result<LogStats, Box<dyn std::error::Error>> {
    let file = File::open(path).await?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();

    let mut stats = LogStats::new();

    while reader.read_line(&mut line).await? > 0 {
        stats.process_line(&line);
        line.clear();
    }

    Ok(stats)
}

#[derive(Debug, Default)]
struct LogStats {
    total_lines: usize,
    error_count: usize,
    warning_count: usize,
    info_count: usize,
}

impl LogStats {
    fn new() -> Self {
        Self::default()
    }

    fn process_line(&mut self, line: &str) {
        self.total_lines += 1;

        if line.contains("ERROR") {
            self.error_count += 1;
        } else if line.contains("WARNING") {
            self.warning_count += 1;
        } else if line.contains("INFO") {
            self.info_count += 1;
        }
    }

    fn print_summary(&self) {
        println!("Log Statistics:");
        println!("  Total lines: {}", self.total_lines);
        println!("  Errors: {}", self.error_count);
        println!("  Warnings: {}", self.warning_count);
        println!("  Info: {}", self.info_count);
    }
}
```

### 异步文件压缩
```rust
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use flate2::write::GzEncoder;
use flate2::read::GzDecoder;
use std::io::Write;

/// 异步文件压缩
async fn compress_file<P: AsRef<Path>>(input_path: P, output_path: P) -> Result<(), Box<dyn std::error::Error>> {
    let input_content = fs::read(&input_path).await?;

    tokio::task::block_in_place(|| {
        let output_file = std::fs::File::create(output_path.as_ref())?;
        let mut encoder = GzEncoder::new(output_file, flate2::Compression::default());
        encoder.write_all(&input_content)?;
        encoder.finish()?;
        Ok::<(), Box<dyn std::error::Error>>(())
    }).await
}

/// 异步文件解压
async fn decompress_file<P: AsRef<Path>>(compressed_path: P, output_path: P) -> Result<(), Box<dyn std::error::Error>> {
    let compressed_content = fs::read(&compressed_path).await?;

    tokio::task::block_in_place(|| {
        let decoder = GzDecoder::new(&compressed_content[..]);
        let mut output = Vec::new();
        decoder.read_to_end(&mut output)?;

        std::fs::write(output_path.as_ref(), output)?;
        Ok::<(), Box<dyn std::error::Error>>(())
    }).await
}

/// 批量压缩文件
async fn batch_compress_files() -> Result<(), Box<dyn std::error::Error>> {
    // 创建测试文件
    for i in 1..=5 {
        let content = format!("This is test content for file {}. ", i);
        for j in 0..1000 {
            let content = format!("{}Line {} ", content, j);
        }
        fs::write(format!("test_{}.txt", i), content).await?;
    }

    // 压缩所有文件
    for i in 1..=5 {
        let input = format!("test_{}.txt", i);
        let output = format!("test_{}.txt.gz", i);

        compress_file(&input, &output).await?;
        println!("Compressed {}", input);
    }

    Ok(())
}
```

## 错误处理和恢复

### 异步I/O错误处理
```rust
use tokio::time::{timeout, Duration};

/// 带超时的异步I/O操作
async fn read_file_with_timeout<P: AsRef<Path>>(path: P, timeout_duration: Duration) -> Result<String, std::io::Error> {
    let read_operation = fs::read_to_string(path);

    match timeout(timeout_duration, read_operation).await {
        Ok(Ok(content)) => Ok(content),
        Ok(Err(e)) => Err(e),
        Err(_) => {
            // 超时错误
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "File read operation timed out"
            ))
        }
    }
}

/// 带重试机制的异步I/O
async fn read_file_with_retry<P: AsRef<Path>>(path: P, max_retries: usize) -> Result<String, Box<dyn std::error::Error>> {
    let mut attempts = 0;

    loop {
        attempts += 1;

        match fs::read_to_string(&path).await {
            Ok(content) => return Ok(content),
            Err(e) if attempts < max_retries => {
                println!("Attempt {} failed: {}. Retrying...", attempts, e);
                tokio::time::sleep(Duration::from_millis(100 * attempts as u64)).await;
                continue;
            }
            Err(e) => {
                println!("Failed after {} attempts: {}", attempts, e);
                return Err(Box::new(e));
            }
        }
    }
}

/// 异步文件备份
async fn backup_file<P: AsRef<Path>>(source: P, backup: P) -> Result<(), Box<dyn std::error::Error>> {
    let source_path = source.as_ref();
    let backup_path = backup.as_ref();

    println!("Backing up {} to {}", source_path.display(), backup_path.display());

    // 尝试读取源文件
    let content = read_file_with_retry(source_path, 3).await?;

    // 创建备份目录（如果需要）
    if let Some(parent) = backup_path.parent() {
        fs::create_dir_all(parent).await?;
    }

    // 写入备份文件
    fs::write(backup_path, content).await?;

    println!("Backup completed successfully");
    Ok(())
}
```

## 相关专题

- `../async/future-trait.md` - Future Trait详解
- `../runtime/executor-design.md` - 执行器设计