# 并发设计模式

## 当使用此专题

- 选择合适的并发设计模式
- 实现高性能的并发架构
- 避免常见的并发陷阱
- 构建可扩展的并发系统

## 核心功能

### 模式选择分析
```
Analyze concurrent pattern: scenario=high_throughput, data_type=messages, throughput=1M_ops, latency=1ms
```
分析最适合的并发设计模式

### 架构模式验证
```
Validate architecture pattern: actors=4, message_passing=true, deadlock_free=true, scalability=linear
```
验证架构模式的正确性

## 生产者-消费者模式

### 多级生产者-消费者
```rust
use std::sync::{Arc, Mutex, Condvar};
use std::thread;
use std::time::Duration;

/// 多级生产者-消费者管道
pub struct PipelineStage<T> {
    input_buffer: Arc<Mutex<Vec<T>>>,
    output_buffer: Arc<Mutex<Vec<T>>>,
    input_ready: Arc<Condvar>,
    output_ready: Arc<Condvar>,
    input_capacity: usize,
    output_capacity: usize,
    processor: Box<dyn Fn(T) -> T + Send + Sync>,
    name: String,
}

impl<T> PipelineStage<T> {
    /// 创建新的管道阶段
    pub fn new<F>(
        name: String,
        input_capacity: usize,
        output_capacity: usize,
        processor: F,
    ) -> Self
    where
        F: Fn(T) -> T + Send + Sync + 'static,
    {
        Self {
            input_buffer: Arc::new(Mutex::new(Vec::with_capacity(input_capacity))),
            output_buffer: Arc::new(Mutex::new(Vec::with_capacity(output_capacity))),
            input_ready: Arc::new(Condvar::new()),
            output_ready: Arc::new(Condvar::new()),
            input_capacity,
            output_capacity,
            processor: Arc::new(processor),
            name,
        }
    }

    /// 启动处理线程
    pub fn start(self) -> PipelineHandle<T> {
        let input_buffer = Arc::clone(&self.input_buffer);
        let output_buffer = Arc::clone(&self.output_buffer);
        let input_ready = Arc::clone(&self.input_ready);
        let output_ready = Arc::clone(&self.output_ready);
        let processor = Arc::clone(&self.processor);
        let name = self.name.clone();

        let handle = thread::spawn(move || {
            let mut processed_count = 0;

            loop {
                let item = {
                    let mut input = input_buffer.lock().unwrap();

                    // 等待输入数据
                    while input.is_empty() {
                        input = input_ready.wait(input).unwrap();
                    }

                    input.remove(0)
                };

                // 处理数据
                let processed_item = (processor)(item);
                processed_count += 1;

                // 将结果放入输出缓冲区
                {
                    let mut output = output_buffer.lock().unwrap();

                    // 等待输出缓冲区有空间
                    while output.len() >= output.capacity() {
                        output = output_ready.wait(output).unwrap();
                    }

                    output.push(processed_item);
                }

                // 通知下游消费者
                output_ready.notify_one();

                // 简单的终止条件
                if processed_count % 10000 == 0 {
                    println!("Stage {} processed {} items", name, processed_count);
                }
            }
        });

        PipelineHandle {
            input_buffer: self.input_buffer,
            output_buffer: self.output_buffer,
            input_ready: self.input_ready,
            output_ready: self.output_ready,
            input_capacity: self.input_capacity,
            handle: Some(handle),
        }
    }
}

/// 管道句柄
pub struct PipelineHandle<T> {
    input_buffer: Arc<Mutex<Vec<T>>>,
    output_buffer: Arc<Mutex<Vec<T>>>,
    input_ready: Arc<Condvar>,
    output_ready: Arc<Condvar>,
    input_capacity: usize,
    handle: Option<thread::JoinHandle<()>>,
}

impl<T> PipelineHandle<T> {
    /// 推送数据到管道
    pub fn push(&self, item: T) -> Result<(), String> {
        let mut input = self.input_buffer.lock().unwrap();

        if input.len() >= self.input_capacity {
            return Err("Input buffer full".to_string());
        }

        input.push(item);
        self.input_ready.notify_one();
        Ok(())
    }

    /// 从管道拉取数据
    pub fn pull(&self) -> Option<T> {
        let mut output = self.output_buffer.lock().unwrap();

        if output.is_empty() {
            None
        } else {
            Some(output.remove(0))
        }
    }

    /// 阻塞式拉取数据
    pub fn pull_blocking(&self, timeout: Duration) -> Option<T> {
        let start_time = std::time::Instant::now();

        while start_time.elapsed() < timeout {
            if let Some(item) = self.pull() {
                return Some(item);
            }
            thread::sleep(Duration::from_millis(1));
        }

        None
    }
}

/// 构建管道
pub fn build_pipeline<T>(stages: Vec<PipelineStage<T>>) -> Vec<PipelineHandle<T>> {
    stages.into_iter().map(|stage| stage.start()).collect()
}

/// 连接管道阶段
pub fn connect_pipeline<T>(handles: &mut [PipelineHandle<T>]) -> Result<(), String> {
    for i in 0..handles.len() - 1 {
        let output = Arc::clone(&handles[i].output_buffer);
        let input = Arc::clone(&handles[i + 1].input_buffer);
        let output_ready = Arc::clone(&handles[i].output_ready);
        let input_ready = Arc::clone(&handles[i + 1].input_ready);

        // 启动转发线程
        thread::spawn(move || {
            loop {
                let item = {
                    let mut output_buf = output.lock().unwrap();

                    if output_buf.is_empty() {
                        drop(output_buf);
                        // 等待数据
                        output_ready.wait(input_ready.lock().unwrap()).unwrap();
                        continue;
                    }

                    output_buf.remove(0)
                };

                {
                    let mut input_buf = input.lock().unwrap();
                    input_buf.push(item);
                }

                input_ready.notify_one();
            }
        });
    }

    Ok(())
}

/// 多级生产者-消费者示例
fn pipeline_example() {
    // 定义处理函数
    let stage1_processor = |x: i32| x * 2; // 乘以2
    let stage2_processor = |x: i32| x + 1; // 加1
    let stage3_processor = |x: i32| x * x; // 平方

    // 创建管道阶段
    let stages = vec![
        PipelineStage::new("Stage1".to_string(), 100, 100, stage1_processor),
        PipelineStage::new("Stage2".to_string(), 100, 100, stage2_processor),
        PipelineStage::new("Stage3".to_string(), 100, 100, stage3_processor),
    ];

    // 构建管道
    let mut handles = build_pipeline(stages);

    // 连接管道阶段
    connect_pipeline(&mut handles).unwrap();

    // 启动生产者线程
    let producer_handle = thread::spawn({
        let handle = Arc::clone(&handles[0]);
        move || {
            for i in 0..10000 {
                if let Err(e) = handle.push(i) {
                    println!("Producer error: {}", e);
                    break;
                }

                // 模拟生产延迟
                if i % 1000 == 0 {
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }
    });

    // 启动消费者线程
    let consumer_handle = thread::spawn({
        let handle = Arc::clone(&handles[handles.len() - 1]);
        move || {
            let mut processed_count = 0;
            let start_time = std::time::Instant::now();

            while processed_count < 10000 {
                if let Some(result) = handle.pull_blocking(Duration::from_secs(1)) {
                    processed_count += 1;

                    // 验证结果
                    let expected = ((processed_count - 1) * 2 + 1).pow(2);
                    assert_eq!(result, expected as i32);

                    if processed_count % 1000 == 0 {
                        println!("Consumer processed {} items", processed_count);
                    }
                }
            }

            let duration = start_time.elapsed();
            println!("Pipeline completed {} items in {:?}", processed_count, duration);
        }
    });

    // 等待完成
    producer_handle.join().unwrap();
    consumer_handle.join().unwrap();
}
```

## Actor模式实现

### Rust Actor框架
```rust
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

/// Actor消息
pub trait Message: Send + 'static {}

/// Actor引用
#[derive(Debug, Clone)]
pub struct ActorRef<M: Message> {
    id: Uuid,
    sender: Arc<Mutex<std::sync::mpsc::Sender<M>>>,
}

impl<M: Message> ActorRef<M> {
    /// 发送消息
    pub fn send(&self, message: M) -> Result<(), String> {
        let sender = self.sender.lock().unwrap();
        sender.send(message).map_err(|e| format!("Send failed: {}", e))
    }

    /// 获取Actor ID
    pub fn id(&self) -> Uuid {
        self.id
    }
}

/// Actor状态
pub trait ActorState<M: Message>: Send {
    fn receive(&mut self, message: M, context: &ActorContext<M>);
    fn started(&mut self, context: &ActorContext<M>) {}
    fn stopped(&mut self, context: &ActorContext<M>) {}
}

/// Actor上下文
pub struct ActorContext<M: Message> {
    id: Uuid,
    system_ref: ActorSystemRef<M>,
}

impl<M: Message> ActorContext<M> {
    pub fn new(id: Uuid, system_ref: ActorSystemRef<M>) -> Self {
        Self { id, system_ref }
    }

    pub fn actor_id(&self) -> Uuid {
        self.id
    }

    pub fn system(&self) -> &ActorSystemRef<M> {
        &self.system_ref
    }

    /// 创建子Actor
    pub fn spawn_actor<S>(&self, state: S) -> Result<ActorRef<M>, String>
    where
        S: ActorState<M> + 'static,
    {
        self.system_ref.spawn_actor(state)
    }

    /// 停止Actor
    pub fn stop_actor(&self, actor_id: Uuid) -> Result<(), String> {
        self.system_ref.stop_actor(actor_id)
    }
}

/// Actor系统引用
#[derive(Clone)]
pub struct ActorSystemRef<M: Message> {
    actors: Arc<Mutex<HashMap<Uuid, std::sync::mpsc::Sender<M>>>>,
}

impl<M: Message> ActorSystemRef<M> {
    pub fn new() -> Self {
        Self {
            actors: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 创建Actor
    pub fn spawn_actor<S>(&self, mut state: S) -> Result<ActorRef<M>, String>
    where
        S: ActorState<M> + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel::<M>();
        let actor_id = Uuid::new_v4();

        let context = ActorContext::new(actor_id, self.clone());
        state.started(&context);

        let actors = Arc::clone(&self.actors);
        {
            let mut actors_map = actors.lock().unwrap();
            actors_map.insert(actor_id, tx.clone());
        }

        let system_ref = self.clone();
        thread::spawn(move || {
            // Actor消息循环
            while let Ok(message) = rx.recv() {
                let context = ActorContext::new(actor_id, system_ref.clone());
                state.receive(message, &context);
            }

            // Actor停止
            let context = ActorContext::new(actor_id, system_ref);
            state.stopped(&context);

            // 从系统中移除
            let mut actors_map = actors.lock().unwrap();
            actors_map.remove(&actor_id);
        });

        Ok(ActorRef {
            id: actor_id,
            sender: Arc::new(Mutex::new(tx)),
        })
    }

    /// 停止Actor
    pub fn stop_actor(&self, actor_id: Uuid) -> Result<(), String> {
        let mut actors = self.actors.lock().unwrap();
        if let Some(sender) = actors.remove(&actor_id) {
            drop(sender); // 关闭通道，Actor线程会退出
            Ok(())
        } else {
            Err(format!("Actor {} not found", actor_id))
        }
    }

    /// 获取活跃Actor数量
    pub fn active_actors(&self) -> usize {
        self.actors.lock().unwrap().len()
    }
}

/// Actor系统
pub struct ActorSystem<M: Message> {
    ref_: ActorSystemRef<M>,
}

impl<M: Message> ActorSystem<M> {
    pub fn new() -> Self {
        Self {
            ref_: ActorSystemRef::new(),
        }
    }

    pub fn spawn_actor<S>(&self, state: S) -> Result<ActorRef<M>, String>
    where
        S: ActorState<M> + 'static,
    {
        self.ref_.spawn_actor(state)
    }

    pub fn stop_actor(&self, actor_id: Uuid) -> Result<(), String> {
        self.ref_.stop_actor(actor_id)
    }

    pub fn active_actors(&self) -> usize {
        self.ref_.active_actors()
    }

    pub fn shutdown(&self) -> Result<(), String> {
        let mut actors = self.ref_.actors.lock().unwrap();
        actors.clear(); // 关闭所有通道
        Ok(())
    }
}

/// 具体的Actor状态示例
#[derive(Debug)]
pub enum CalculatorMessage {
    Add(i32, i32),
    Multiply(i32, i32),
    GetResult,
}

impl Message for CalculatorMessage {}

pub struct CalculatorActor {
    result: i32,
    requester: Option<ActorRef<CalculatorMessage>>,
}

impl CalculatorActor {
    pub fn new() -> Self {
        Self {
            result: 0,
            requester: None,
        }
    }
}

impl ActorState<CalculatorMessage> for CalculatorActor {
    fn receive(&mut self, message: CalculatorMessage, _context: &ActorContext<CalculatorMessage>) {
        match message {
            CalculatorMessage::Add(a, b) => {
                self.result = a + b;
                if let Some(ref requester) = self.requester {
                    let _ = requester.send(CalculatorMessage::GetResult);
                }
            }
            CalculatorMessage::Multiply(a, b) => {
                self.result = a * b;
                if let Some(ref requester) = self.requester {
                    let _ = requester.send(CalculatorMessage::GetResult);
                }
            }
            CalculatorMessage::GetResult => {
                println!("Calculator result: {}", self.result);
            }
        }
    }

    fn started(&mut self, context: &ActorContext<CalculatorMessage>) {
        println!("CalculatorActor {} started", context.actor_id());
    }

    fn stopped(&mut self, context: &ActorContext<CalculatorMessage>) {
        println!("CalculatorActor {} stopped", context.actor_id());
    }
}

/// 主控Actor状态
#[derive(Debug)]
pub enum MasterMessage {
    StartCalculation,
    CalculatorResult(i32),
    Done,
}

impl Message for MasterMessage {}

pub struct MasterActor {
    calculator: Option<ActorRef<CalculatorMessage>>,
    completed_calculations: usize,
}

impl MasterActor {
    pub fn new() -> Self {
        Self {
            calculator: None,
            completed_calculations: 0,
        }
    }
}

impl ActorState<MasterMessage> for MasterActor {
    fn receive(&mut self, message: MasterMessage, context: &ActorContext<MasterMessage>) {
        match message {
            MasterMessage::StartCalculation => {
                if self.calculator.is_none() {
                    match context.spawn_actor(CalculatorActor::new()) {
                        Ok(calculator) => {
                            self.calculator = Some(calculator);
                            println!("Created calculator actor");
                        }
                        Err(e) => println!("Failed to create calculator: {}", e),
                    }
                }

                if let Some(ref calculator) = self.calculator {
                    // 发送计算任务
                    for i in 0..10 {
                        let _ = calculator.send(CalculatorMessage::Add(i, i * 2));
                        let _ = calculator.send(CalculatorMessage::Multiply(i, i + 1));
                    }
                }
            }
            MasterMessage::CalculatorResult(result) => {
                println!("Master received calculator result: {}", result);
                self.completed_calculations += 1;

                if self.completed_calculations >= 20 {
                    if let Some(ref calculator) = self.calculator {
                        let _ = calculator.send(CalculatorMessage::GetResult);
                    }
                    let _ = context.system().stop_actor(context.actor_id());
                }
            }
            MasterMessage::Done => {
                println!("Master actor finished all calculations");
            }
        }
    }

    fn started(&mut self, context: &ActorContext<MasterMessage>) {
        println!("MasterActor {} started", context.actor_id());

        // 启动计算
        self.receive(MasterMessage::StartCalculation, context);
    }

    fn stopped(&mut self, context: &ActorContext<MasterMessage>) {
        println!("MasterActor {} stopped", context.actor_id());
    }
}

/// Actor模式使用示例
fn actor_pattern_example() {
    let system = ActorSystem::<MasterMessage>::new();

    // 创建主控Actor
    let master = system.spawn_actor(MasterActor::new()).unwrap();

    println!("Actor system started with {} actors", system.active_actors());

    // 运行一段时间
    thread::sleep(Duration::from_secs(2));

    println!("Shutting down actor system...");
    let _ = system.shutdown();

    thread::sleep(Duration::from_millis(500));
    println!("Actor system shutdown complete");
}
```

## 工作窃取模式

### 高级工作窃取
```rust
use std::sync::{Arc, Mutex, Condvar};
use std::collections::VecDeque;
use std::thread;
use std::time::Duration;

/// 工作窃取任务
pub trait Task: Send + 'static {
    fn run(self: Box<Self>);
}

impl<F> Task for F
where
    F: FnOnce() + Send + 'static,
{
    fn run(self: Box<Self>) {
        (*self)()
    }
}

/// 工作窃取队列
pub struct WorkStealingQueue {
    tasks: Arc<Mutex<VecDeque<Box<dyn Task>>>>,
    steal_condvar: Arc<Condvar>,
    worker_id: usize,
}

impl WorkStealingQueue {
    pub fn new(worker_id: usize) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(VecDeque::new())),
            steal_condvar: Arc::new(Condvar::new()),
            worker_id,
        }
    }

    /// 添加任务到本地队列
    pub fn push_local(&self, task: Box<dyn Task>) {
        let mut tasks = self.tasks.lock().unwrap();
        tasks.push_back(task);
    }

    /// 从本地队列获取任务
    pub fn pop_local(&self) -> Option<Box<dyn Task>> {
        let mut tasks = self.tasks.lock().unwrap();
        tasks.pop_front()
    }

    /// 从队列尾部获取任务（LIFO）
    pub fn pop_back(&self) -> Option<Box<dyn Task>> {
        let mut tasks = self.tasks.lock().unwrap();
        tasks.pop_back()
    }

    /// 窃取任务（从队列头部，FIFO）
    pub fn steal(&self) -> Option<Box<dyn Task>> {
        let mut tasks = self.tasks.lock().unwrap();
        tasks.pop_front()
    }

    /// 通知有新任务
    pub fn notify(&self) {
        self.steal_condvar.notify_one();
    }

    /// 等待任务
    pub fn wait_for_task(&self, timeout: Duration) -> bool {
        let tasks = self.tasks.lock().unwrap();
        let _guard = self.steal_condvar.wait_timeout(tasks, timeout).unwrap();
        true
    }

    /// 获取任务数量
    pub fn len(&self) -> usize {
        let tasks = self.tasks.lock().unwrap();
        tasks.len()
    }
}

/// 工作窃取线程池
pub struct WorkStealingThreadPool {
    workers: Vec<WorkStealingWorker>,
    steal_queues: Vec<Arc<WorkStealingQueue>>,
    global_queue: Arc<Mutex<VecDeque<Box<dyn Task>>>>,
    shutdown: Arc<Mutex<bool>>,
}

struct WorkStealingWorker {
    id: usize,
    local_queue: Arc<WorkStealingQueue>,
    steal_queues: Vec<Arc<WorkStealingQueue>>,
    global_queue: Arc<Mutex<VecDeque<Box<dyn Task>>>>,
    shutdown: Arc<Mutex<bool>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl WorkStealingThreadPool {
    pub fn new(num_workers: usize) -> Self {
        let mut steal_queues = Vec::new();
        let mut workers = Vec::new();

        // 创建工作窃取队列
        for i in 0..num_workers {
            let queue = Arc::new(WorkStealingQueue::new(i));
            steal_queues.push(queue);
        }

        let global_queue = Arc::new(Mutex::new(VecDeque::new()));
        let shutdown = Arc::new(Mutex::new(false));

        // 创建工作线程
        for i in 0..num_workers {
            let local_queue = Arc::clone(&steal_queues[i]);
            let mut worker_steal_queues = Vec::new();

            for (j, queue) in steal_queues.iter().enumerate() {
                if j != i {
                    worker_steal_queues.push(Arc::clone(queue));
                }
            }

            let global_queue_clone = Arc::clone(&global_queue);
            let shutdown_clone = Arc::clone(&shutdown);

            let worker = WorkStealingWorker {
                id: i,
                local_queue,
                steal_queues: worker_steal_queues,
                global_queue: global_queue_clone,
                shutdown: shutdown_clone,
                handle: None,
            };

            workers.push(worker);
        }

        WorkStealingThreadPool {
            workers,
            steal_queues,
            global_queue,
            shutdown,
        }
    }

    /// 启动线程池
    pub fn start(&mut self) {
        for worker in &mut self.workers {
            let mut local_queue = Arc::clone(&worker.local_queue);
            let steal_queues = worker.steal_queues.clone();
            let global_queue = Arc::clone(&worker.global_queue);
            let shutdown = Arc::clone(&worker.shutdown);
            let worker_id = worker.id;

            let handle = thread::spawn(move || {
                let mut tasks_executed = 0;

                while !*shutdown.lock().unwrap() {
                    let mut task = None;

                    // 1. 尝试从本地队列获取任务（LIFO）
                    task = local_queue.pop_back();

                    // 2. 尝试从全局队列获取任务
                    if task.is_none() {
                        let mut global = global_queue.lock().unwrap();
                        task = global.pop_front();
                    }

                    // 3. 尝试从其他线程窃取任务（FIFO）
                    if task.is_none() {
                        // 随机化窃取顺序以避免热点
                        let mut indices: Vec<usize> = (0..steal_queues.len()).collect();
                        use rand::seq::SliceRandom;
                        indices.shuffle(&mut rand::thread_rng());

                        for &i in &indices {
                            if let Some(stolen_task) = steal_queues[i].steal() {
                                task = Some(stolen_task);
                                break;
                            }
                        }
                    }

                    // 4. 执行任务
                    if let Some(task) = task {
                        task.run();
                        tasks_executed += 1;

                        // 定期输出统计信息
                        if tasks_executed % 10000 == 0 {
                            println!("Worker {} executed {} tasks", worker_id, tasks_executed);
                        }
                    } else {
                        // 没有任务时短暂休眠
                        thread::sleep(Duration::from_millis(1));
                    }
                }

                println!("Worker {} shutting down after {} tasks", worker_id, tasks_executed);
            });

            worker.handle = Some(handle);
        }
    }

    /// 提交任务到本地队列
    pub fn submit_local<F>(&self, task: F, worker_id: usize) -> Result<(), String>
    where
        F: FnOnce() + Send + 'static,
    {
        if worker_id >= self.steal_queues.len() {
            return Err(format!("Invalid worker id: {}", worker_id));
        }

        let boxed_task: Box<dyn Task> = Box::new(task);
        self.steal_queues[worker_id].push_local(boxed_task);
        self.steal_queues[worker_id].notify();

        Ok(())
    }

    /// 提交任务到全局队列
    pub fn submit<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let boxed_task: Box<dyn Task> = Box::new(task);
        let mut global = self.global_queue.lock().unwrap();
        global.push_back(boxed_task);

        // 通知一个空闲线程
        if global.len() == 1 {
            drop(global);
            for queue in &self.steal_queues {
                queue.notify();
            }
        }
    }

    /// 获取负载统计
    pub fn get_load_stats(&self) -> Vec<(usize, usize)> {
        self.steal_queues
            .iter()
            .enumerate()
            .map(|(id, queue)| (id, queue.len()))
            .collect()
    }

    /// 关闭线程池
    pub fn shutdown(&mut self) {
        *self.shutdown.lock().unwrap() = true;

        for worker in &mut self.workers {
            if let Some(handle) = worker.handle.take() {
                handle.join().unwrap();
            }
        }
    }
}

/// 工作窃取使用示例
fn work_stealing_example() {
    let mut pool = WorkStealingThreadPool::new(4);
    pool.start();

    // 提交大量计算任务
    for i in 0..100000 {
        pool.submit(move || {
            // 模拟计算密集型任务
            let mut result = 0u64;
            for j in 0..1000 {
                result = result.wrapping_add((i * 1000 + j) as u64);
            }
            result
        });
    }

    // 提交到特定工作线程
    for i in 0..10000 {
        let worker_id = i % 4;
        let _ = pool.submit_local(move || {
            // 模拟I/O密集型任务
            thread::sleep(Duration::from_micros(10));
            i * i
        }, worker_id);
    }

    // 监控负载分布
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(500));
        let stats = pool.get_load_stats();
        println!("Load distribution: {:?}", stats);
    }

    thread::sleep(Duration::from_secs(1));

    println!("Shutting down work stealing pool...");
    pool.shutdown();
    println!("Work stealing pool shutdown complete");
}
```

## 相关专题

- `../threading/work-stealing.md` - 工作窃取算法详解
- `../datastructures/lockfree-queue.md` - 无锁数据结构
- `../sync/mutex-rwlock.md` - 同步原语
- `../atomic/memory-ordering.md` - 原子操作