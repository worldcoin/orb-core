use orb::{
    pid,
    pid::{Pid, Timer},
};
use std::fs;

// Simulates an object in one-dimensional space having mass, momentum, and
// friction. The actor applies force to the object.
struct Simulator {
    mass: f64,
    friction: f64,
    y: f64,
    speed: f64,
}

impl Simulator {
    fn new(mass: f64, friction: f64, y: f64) -> Self {
        Self { mass, friction, y, speed: 0.0 }
    }

    fn apply(&mut self, force: f64, dt: f64) -> f64 {
        self.speed += force * dt / self.mass;
        self.speed *= 1.0 - self.friction * dt;
        self.y += self.speed;
        self.y
    }
}

#[test]
fn test_pid() {
    const DT: f64 = 0.01;
    const SETPOINT: f64 = 1000.0;
    let mut pid = Pid::default().with_proportional(2.0).with_derivative(1.0);
    let mut y = 0.0;
    let mut sim = Simulator::new(40.0, 1.0, y);
    let mut timer = pid::ConstDelta::from(DT);
    let mut setpoint_history = Vec::new();
    let mut process_history = Vec::new();
    let mut control_history = Vec::new();
    for _ in 0..750 {
        let dt = timer.get_dt().unwrap_or(0.0);
        let control = pid.advance(SETPOINT, y, dt).clamp(-100.0, 100.0);
        y = sim.apply(control, DT);
        setpoint_history.push(SETPOINT);
        process_history.push(y);
        control_history.push(control);
    }
    assert_gnuplot("pid", &setpoint_history, &process_history, &control_history);
}

fn assert_gnuplot(
    name: &str,
    setpoint_history: &[f64],
    process_history: &[f64],
    control_history: &[f64],
) {
    let render = |mut acc: String, (t, y)| {
        acc.push_str(&format!("{t} {y:.03}\n"));
        acc
    };
    let setpoint_data = setpoint_history.iter().enumerate().fold(String::new(), render);
    let process_data = process_history.iter().enumerate().fold(String::new(), render);
    let control_data = control_history.iter().enumerate().fold(String::new(), render);
    let result = format!(
        r#"$setpoint << EOD
{setpoint_data}EOD
$process << EOD
{process_data}EOD
$control << EOD
{control_data}EOD
set y2tics
set ytics nomirror
plot "$control" with lines axes x1y2, "$process" with lines, "$setpoint" with lines
"#
    );
    if fs::read_to_string(format!("tests/pid/{name}.gnuplot")).unwrap_or_default() != result {
        fs::write(format!("tests/pid/{name}.gnuplot.unmatched"), result.as_bytes()).unwrap();
        panic!("tests/pid/{name}.gnuplot doesn't match");
    }
}
