macro_rules! clone_expr {
    ($($var:ident),+ => $expr:expr) => {{
        $(
            let $var = $var.clone();
        )+
        $expr
    }};
}

pub(crate) use clone_expr;

// macro_rules! perf_ms {
//     ($expr:expr) => {{
//         let now = std::time::Instant::now();
//         let res = { $expr };
//         println!("ms: {}", (now.elapsed().as_micros() as f64) / 1000.0);
//         res
//     }};
// }
//
// pub(crate) use perf_ms;
