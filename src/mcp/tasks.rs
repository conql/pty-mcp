use std::sync::Arc;

use rmcp::task_manager::OperationProcessor;
use tokio::sync::Mutex;

pub type TaskProcessor = Arc<Mutex<OperationProcessor>>;

pub fn new_task_processor() -> TaskProcessor {
    Arc::new(Mutex::new(OperationProcessor::new()))
}
