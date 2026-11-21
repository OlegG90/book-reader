//! Outside Windows the process's own standard output is the console (see console.rs).

/// Prints to standard output.
pub fn print(text: &str) {
    println!("\n{text}");
}
