macro_rules! clone_expr {
    ($($var:ident),+ => $expr:expr) => {{
        $(
            let $var = $var.clone();
        )+
        $expr
    }};
}

macro_rules! dbg_err {
    ($expr:expr) => {{
        let res = $expr;
        if let Err(v) = &res {
            dbg!(v);
        }
        res
    }};
}

pub(crate) use clone_expr;
pub(crate) use dbg_err;

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
