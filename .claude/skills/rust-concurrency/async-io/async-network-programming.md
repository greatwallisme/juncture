# 异步网络编程

## 当使用此专题

- 构建高性能异步网络服务
- 实现可扩展的TCP/UDP服务器
- 优化网络I/O性能
- 处理大规模并发连接

## 异步TCP服务器

### 高性能TCP服务器
```rust
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use std::sync::Arc;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// TCP服务器配置
#[derive(Debug, Clone)]
pub struct TcpServerConfig {
    pub bind_addr: String,
    pub max_connections: usize,
    pub keepalive_timeout: Duration,
    pub buffer_size: usize,
    pub worker_threads: usize,
}

impl Default for TcpServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:8080".to_string(),
            max_connections: 10000,
            keepalive_timeout: Duration::from_secs(30),
            buffer_size: 8192,
            worker_threads: num_cpus::get(),
        }
    }
}

/// 连接统计信息
#[derive(Debug, Default)]
pub struct ConnectionStats {
    pub total_connections: AtomicUsize,
    pub active_connections: AtomicUsize,
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub errors: AtomicUsize,
    pub last_reset: AtomicInstant,
}

impl ConnectionStats {
    pub fn new() -> Self {
        Self {
            last_reset: AtomicInstant::new(Instant::now()),
            ..Default::default()
        }
    }

    pub fn reset(&self) {
        self.total_connections.store(0, Ordering::Relaxed);
        self.active_connections.store(0, Ordering::Relaxed);
        self.bytes_sent.store(0, Ordering::Relaxed);
        self.bytes_received.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
        self.last_reset.store(Instant::now(), Ordering::Relaxed);
    }

    pub fn print_stats(&self) {
        let elapsed = self.last_reset.load(Ordering::Relaxed).elapsed();
        let total_conns = self.total_connections.load(Ordering::Relaxed);
        let active_conns = self.active_connections.load(Ordering::Relaxed);
        let bytes_sent = self.bytes_sent.load(Ordering::Relaxed);
        let bytes_received = self.bytes_received.load(Ordering::Relaxed);
        let errors = self.errors.load(Ordering::Relaxed);

        println!("=== TCP Server Statistics ===");
        println!("Uptime: {:?}", elapsed);
        println!("Total Connections: {}", total_conns);
        println!("Active Connections: {}", active_conns);
        println!("Bytes Sent: {:.2} MB", bytes_sent as f64 / (1024.0 * 1024.0));
        println!("Bytes Received: {:.2} MB", bytes_received as f64 / (1024.0 * 1024.0));
        println!("Errors: {}", errors);

        if elapsed.as_secs() > 0 {
            let send_rate = bytes_sent as f64 / elapsed.as_secs_f64();
            let recv_rate = bytes_received as f64 / elapsed.as_secs_f64();
            println!("Send Rate: {:.2} MB/s", send_rate / (1024.0 * 1024.0));
            println!("Receive Rate: {:.2} MB/s", recv_rate / (1024.0 * 1024.0));
        }
    }
}

/// 异步TCP服务器
pub struct AsyncTcpServer {
    config: TcpServerConfig,
    stats: Arc<ConnectionStats>,
    shutdown: Arc<AtomicBool>,
}

impl AsyncTcpServer {
    pub fn new(config: TcpServerConfig) -> Self {
        Self {
            stats: Arc::new(ConnectionStats::new()),
            shutdown: Arc::new(AtomicBool::new(false)),
            config,
        }
    }

    /// 启动服务器
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("Starting TCP server on {}", self.config.bind_addr);

        let listener = TcpListener::bind(&self.config.bind_addr).await?;
        println!("TCP server listening on {}", self.config.bind_addr);

        let stats = Arc::clone(&self.stats);
        let shutdown = Arc::clone(&self.shutdown);
        let keepalive_timeout = self.config.keepalive_timeout;

        // 启动统计监控线程
        let stats_monitor = Arc::clone(&self.stats);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                stats_monitor.print_stats();
            }
        });

        // 主接受循环
        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((socket, addr)) => {
                            stats.total_connections.fetch_add(1, Ordering::Relaxed);
                            stats.active_connections.fetch_add(1, Ordering::Relaxed);

                            println!("New connection from {}", addr);

                            // 处理连接
                            let stats = Arc::clone(&stats);
                            let shutdown_clone = Arc::clone(&shutdown);
                            let buffer_size = self.config.buffer_size;

                            tokio::spawn(async move {
                                let result = Self::handle_connection(
                                    socket,
                                    addr,
                                    stats,
                                    shutdown_clone,
                                    keepalive_timeout,
                                    buffer_size,
                                ).await;

                                stats.active_connections.fetch_sub(1, Ordering::Relaxed);

                                if let Err(e) = result {
                                    eprintln!("Connection handler error: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("Error accepting connection: {}", e);
                            stats.errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }

                _ = tokio::signal::ctrl_c() => {
                    println!("Received shutdown signal");
                    shutdown.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }

        println!("TCP server shutdown");
        Ok(())
    }

    /// 处理连接
    async fn handle_connection(
        mut socket: TcpStream,
        addr: std::net::SocketAddr,
        stats: Arc<ConnectionStats>,
        shutdown: Arc<AtomicBool>,
        keepalive_timeout: Duration,
        buffer_size: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut last_activity = Instant::now();
        let (reader, mut writer) = socket.split();
        let mut reader = BufReader::with_capacity(buffer_size, reader);
        let mut writer = BufWriter::new(&mut writer);

        // 设置TCP选项
        socket.set_keepalive(Some(keepalive_timeout))?;
        socket.set_nodelay(true)?;

        loop {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }

            // 检查超时
            if last_activity.elapsed() > keepalive_timeout {
                println!("Connection {} timeout", addr);
                break;
            }

            // 设置读超时
            let read_future = tokio::time::timeout(
                Duration::from_secs(5),
                reader.fill_buf()
            );

            match read_future.await {
                Ok(Ok(buf)) if buf.is_empty() => {
                    // 连接关闭
                    break;
                }
                Ok(Ok(buf)) => {
                    let received = buf.len();
                    stats.bytes_received.fetch_add(received as u64, Ordering::Relaxed);
                    last_activity = Instant::now();

                    // 简单的回显服务器
                    let response = format!("Echo from server ({} bytes): ", received);
                    writer.write_all(response.as_bytes()).await?;
                    writer.write_all(buf).await?;
                    writer.flush().await?;

                    let sent = response.len() + buf.len();
                    stats.bytes_sent.fetch_add(sent as u64, Ordering::Relaxed);

                    reader.consume(received);
                }
                Ok(Err(e)) => {
                    eprintln!("Read error: {}", e);
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                    break;
                }
                Err(_) => {
                    // 超时，继续循环检查关闭信号
                    continue;
                }
            }
        }

        println!("Connection {} closed", addr);
        Ok(())
    }

    /// 获取统计信息
    pub fn get_stats(&self) -> &ConnectionStats {
        &self.stats
    }

    /// 停止服务器
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// 高级连接处理器
pub struct ConnectionHandler {
    request_handlers: HashMap<String, Box<dyn RequestHandler + Send + Sync>>,
    stats: Arc<ConnectionStats>,
}

pub trait RequestHandler: Send + Sync {
    fn handle(&self, request: &[u8]) -> Vec<u8>;
    fn name(&self) -> &str;
}

impl ConnectionHandler {
    pub fn new(stats: Arc<ConnectionStats>) -> Self {
        Self {
            request_handlers: HashMap::new(),
            stats,
        }
    }

    pub fn register_handler<H>(&mut self, path: &str, handler: H)
    where
        H: RequestHandler + 'static,
    {
        self.request_handlers.insert(path.to_string(), Box::new(handler));
    }

    pub async fn handle_http_connection(
        &self,
        socket: TcpStream,
        addr: std::net::SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (mut reader, mut writer) = socket.into_split();
        let mut buffer = Vec::new();

        // 读取HTTP请求
        let mut read_buf = [0u8; 4096];
        loop {
            let bytes_read = reader.read(&mut read_buf).await?;
            if bytes_read == 0 {
                break;
            }

            buffer.extend_from_slice(&read_buf[..bytes_read]);

            // 检查是否收到完整的HTTP头
            if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }

            if buffer.len() > 8192 {
                // 请求太大
                let response = "HTTP/1.1 413 Request Entity Too Large\r\n\r\n";
                writer.write_all(response.as_bytes()).await?;
                return Ok(());
            }
        }

        // 解析HTTP请求
        let request = String::from_utf8_lossy(&buffer);
        let lines: Vec<&str> = request.lines().collect();

        if lines.is_empty() {
            return Ok(());
        }

        let request_line = lines[0];
        let parts: Vec<&str> = request_line.split_whitespace().collect();

        if parts.len() < 2 {
            return Ok(());
        }

        let method = parts[0];
        let path = parts[1];

        // 处理请求
        let response = if let Some(handler) = self.request_handlers.get(path) {
            let request_data = format!("{} {}", method, path);
            let result = handler.handle(request_data.as_bytes());

            format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/plain\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\r\n\
                 {}",
                result.len(),
                String::from_utf8_lossy(&result)
            )
        } else {
            "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found".to_string()
        };

        writer.write_all(response.as_bytes()).await?;
        writer.flush().await?;

        println!("HTTP {} {} - {} - 200 OK", method, path, addr);
        Ok(())
    }
}

/// 示例请求处理器
pub struct EchoHandler;

impl RequestHandler for EchoHandler {
    fn handle(&self, request: &[u8]) -> Vec<u8> {
        let request_str = String::from_utf8_lossy(request);
        format!("Echo: {}", request_str).into_bytes()
    }

    fn name(&self) -> &str {
        "EchoHandler"
    }
}

pub struct TimeHandler;

impl RequestHandler for TimeHandler {
    fn handle(&self, _request: &[u8]) -> Vec<u8> {
        let now = std::time::SystemTime::now();
        format!("Server time: {:?}", now).into_bytes()
    }

    fn name(&self) -> &str {
        "TimeHandler"
    }
}

/// 异步TCP服务器使用示例
async fn tcp_server_example() -> Result<(), Box<dyn std::error::Error>> {
    let config = TcpServerConfig {
        bind_addr: "127.0.0.1:8080".to_string(),
        max_connections: 1000,
        keepalive_timeout: Duration::from_secs(60),
        buffer_size: 16384,
        worker_threads: 4,
    };

    let server = AsyncTcpServer::new(config);

    // 启动服务器（这会阻塞直到收到关闭信号）
    server.start().await?;

    Ok(())
}
```

## 异步UDP服务器

### 高性能UDP服务器
```rust
use tokio::net::UdpSocket;
use std::net::SocketAddr;
use std::sync::Arc;
use std::collections::HashMap;
use std::time::Instant;

/// UDP服务器配置
#[derive(Debug, Clone)]
pub struct UdpServerConfig {
    pub bind_addr: String,
    pub buffer_size: usize,
    pub max_packet_size: usize,
    pub worker_threads: usize,
}

impl Default for UdpServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:9090".to_string(),
            buffer_size: 65536,
            max_packet_size: 4096,
            worker_threads: num_cpus::get(),
        }
    }
}

/// UDP连接管理器
pub struct UdpConnectionManager {
    connections: Arc<Mutex<HashMap<SocketAddr, ConnectionInfo>>>,
    stats: Arc<UdpStats>,
}

#[derive(Debug)]
struct ConnectionInfo {
    first_seen: Instant,
    last_seen: Instant,
    packets_sent: u64,
    packets_received: u64,
    bytes_sent: u64,
    bytes_received: u64,
}

impl UdpConnectionManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            stats: Arc::new(UdpStats::new()),
        }
    }

    pub fn update_connection(&self, addr: SocketAddr, bytes_received: usize) {
        let mut connections = self.connections.lock().unwrap();
        let now = Instant::now();

        let info = connections.entry(addr).or_insert(ConnectionInfo {
            first_seen: now,
            last_seen: now,
            packets_sent: 0,
            packets_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
        });

        info.last_seen = now;
        info.packets_received += 1;
        info.bytes_received += bytes_received as u64;

        self.stats.packets_received.fetch_add(1, Ordering::Relaxed);
        self.stats.bytes_received.fetch_add(bytes_received as u64, Ordering::Relaxed);
    }

    pub fn record_sent(&self, addr: SocketAddr, bytes_sent: usize) {
        if let Some(info) = self.connections.lock().unwrap().get_mut(&addr) {
            info.packets_sent += 1;
            info.bytes_sent += bytes_sent as u64;

            self.stats.packets_sent.fetch_add(1, Ordering::Relaxed);
            self.stats.bytes_sent.fetch_add(bytes_sent as u64, Ordering::Relaxed);
        }
    }

    pub fn cleanup_inactive(&self, timeout: Duration) {
        let mut connections = self.connections.lock().unwrap();
        let now = Instant::now();

        connections.retain(|_, info| {
            now.duration_since(info.last_seen) < timeout
        });
    }

    pub fn print_stats(&self) {
        let connections = self.connections.lock().unwrap();
        let total_connections = connections.len();
        let total_packets_received = self.stats.packets_received.load(Ordering::Relaxed);
        let total_packets_sent = self.stats.packets_sent.load(Ordering::Relaxed);
        let total_bytes_received = self.stats.bytes_received.load(Ordering::Relaxed);
        let total_bytes_sent = self.stats.bytes_sent.load(Ordering::Relaxed);
        let total_errors = self.stats.errors.load(Ordering::Relaxed);

        println!("=== UDP Server Statistics ===");
        println!("Active Connections: {}", total_connections);
        println!("Packets Received: {}", total_packets_received);
        println!("Packets Sent: {}", total_packets_sent);
        println!("Bytes Received: {:.2} MB", total_bytes_received as f64 / (1024.0 * 1024.0));
        println!("Bytes Sent: {:.2} MB", total_bytes_sent as f64 / (1024.0 * 1024.0));
        println!("Errors: {}", total_errors);

        if !connections.is_empty() {
            println!("\nConnection Details:");
            for (addr, info) in connections.iter() {
                let duration = info.last_seen.duration_since(info.first_seen);
                println!("  {} - Packets: {}/{}, Bytes: {}/{}, Duration: {:?}",
                    addr,
                    info.packets_received, info.packets_sent,
                    info.bytes_received, info.bytes_sent,
                    duration
                );
            }
        }
    }
}

/// UDP统计信息
#[derive(Debug)]
pub struct UdpStats {
    packets_received: AtomicU64,
    packets_sent: AtomicU64,
    bytes_received: AtomicU64,
    bytes_sent: AtomicU64,
    errors: AtomicU64,
}

impl UdpStats {
    pub fn new() -> Self {
        Self {
            packets_received: AtomicU64::new(0),
            packets_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }
}

/// 异步UDP服务器
pub struct AsyncUdpServer {
    config: UdpServerConfig,
    connection_manager: Arc<UdpConnectionManager>,
    shutdown: Arc<AtomicBool>,
}

impl AsyncUdpServer {
    pub fn new(config: UdpServerConfig) -> Self {
        Self {
            connection_manager: Arc::new(UdpConnectionManager::new()),
            shutdown: Arc::new(AtomicBool::new(false)),
            config,
        }
    }

    /// 启动UDP服务器
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let socket = UdpSocket::bind(&self.config.bind_addr).await?;
        println!("UDP server listening on {}", self.config.bind_addr);

        let connection_manager = Arc::clone(&self.connection_manager);
        let shutdown = Arc::clone(&self.shutdown);
        let max_packet_size = self.config.max_packet_size;

        // 启动统计监控线程
        let manager = Arc::clone(&self.connection_manager);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));

            loop {
                interval.tick().await;
                manager.print_stats();

                // 清理不活跃的连接
                manager.cleanup_inactive(Duration::from_secs(300));
            }
        });

        // 主接收循环
        let mut buffer = vec![0u8; max_packet_size];

        loop {
            tokio::select! {
                result = socket.recv_from(&mut buffer) => {
                    match result {
                        Ok((len, addr)) => {
                            let packet_data = buffer[..len].to_vec();
                            let data_len = packet_data.len();

                            // 更新连接信息
                            connection_manager.update_connection(addr, data_len);

                            // 处理数据包
                            let response = self.process_packet(&packet_data, addr);

                            // 发送响应
                            if let Some(response_data) = response {
                                match socket.send_to(&response_data, addr).await {
                                    Ok(sent) => {
                                        connection_manager.record_sent(addr, sent);
                                    }
                                    Err(e) => {
                                        eprintln!("Error sending response to {}: {}", addr, e);
                                        connection_manager.stats.errors.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error receiving UDP packet: {}", e);
                            connection_manager.stats.errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }

                _ = tokio::signal::ctrl_c() => {
                    println!("Received shutdown signal");
                    shutdown.store(true, Ordering::Relaxed);
                    break;
                }
            }

            if shutdown.load(Ordering::Relaxed) {
                break;
            }
        }

        println!("UDP server shutdown");
        Ok(())
    }

    /// 处理数据包
    fn process_packet(&self, packet: &[u8], addr: SocketAddr) -> Option<Vec<u8>> {
        // 简单的回显服务
        let request = String::from_utf8_lossy(packet);
        println!("Received from {}: {}", addr, request.trim());

        if request.trim().starts_with("ECHO") {
            let response = format!("ECHO: {}", request.trim()[4..].trim());
            Some(response.into_bytes())
        } else if request.trim().starts_with("TIME") {
            let now = std::time::SystemTime::now();
            let response = format!("TIME: {:?}", now);
            Some(response.into_bytes())
        } else if request.trim().starts_with("PING") {
            Some("PONG".to_string().into_bytes())
        } else if request.trim().starts_with("STATS") {
            let mut output = Vec::new();
            use std::io::Write;

            let connections = self.connection_manager.connections.lock().unwrap();
            writeln!(output, "Active connections: {}", connections.len()).unwrap();

            for (conn_addr, info) in connections.iter() {
                writeln!(output, "{}: {} packets received, {} sent",
                    conn_addr, info.packets_received, info.packets_sent).unwrap();
            }

            Some(output)
        } else {
            // 默认响应
            let response = format!("Unknown command: {}", request.trim());
            Some(response.into_bytes())
        }
    }

    /// 获取连接管理器
    pub fn connection_manager(&self) -> &UdpConnectionManager {
        &self.connection_manager
    }

    /// 停止服务器
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// UDP服务器使用示例
async fn udp_server_example() -> Result<(), Box<dyn std::error::Error>> {
    let config = UdpServerConfig {
        bind_addr: "127.0.0.1:9090".to_string(),
        buffer_size: 65536,
        max_packet_size: 8192,
        worker_threads: 4,
    };

    let server = AsyncUdpServer::new(config);

    // 启动服务器
    server.start().await?;

    Ok(())
}

/// 测试客户端
async fn udp_test_client() -> Result<(), Box<dyn std::error::Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let server_addr: SocketAddr = "127.0.0.1:9090".parse().unwrap();

    let test_commands = vec![
        "ECHO Hello, World!",
        "TIME",
        "PING",
        "STATS",
        "INVALID COMMAND",
    ];

    for command in test_commands {
        println!("Sending: {}", command);
        socket.send_to(command.as_bytes(), server_addr).await?;

        let mut buffer = [0u8; 1024];
        match socket.recv_from(&mut buffer).await {
            Ok((len, _)) => {
                let response = String::from_utf8_lossy(&buffer[..len]);
                println!("Received: {}", response.trim());
            }
            Err(e) => {
                eprintln!("Error receiving response: {}", e);
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Ok(())
}
```

##  相关专题

- `../async-io/async-file-operations.md` - 异步文件操作
- `../runtime/executor-design.md` - 异步运行时设计
- `../performance/cache-optimization.md` - 性能优化
- `../patterns/concurrent-patterns.md` - 并发设计模式