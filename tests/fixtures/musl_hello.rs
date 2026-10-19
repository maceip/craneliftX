fn main() {
    let s = String::from("hello from real static musl");
    // write directly to stdout fd 1
    println!("{}", s);
}
