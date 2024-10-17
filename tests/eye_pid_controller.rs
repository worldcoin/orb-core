use orb::{agents::eye_pid_controller::EyeOffsetController, pid, pid::Timer};
use std::fs;

#[test]
fn test_pid() {
    const DT: f64 = 0.5;
    let mut timer = pid::ConstDelta::from(DT);
    let mut controller = EyeOffsetController::new(0.5);
    let mut setpoint_history = Vec::new();
    let mut process_history = Vec::new();
    let mut process = 0.0;
    let mut setpoint = 20.0;
    for t in 0..600 {
        if t == 100 {
            setpoint = 10.0;
        } else if t == 200 {
            setpoint = 0.0;
        } else if t == 300 {
            setpoint = 30.0;
        } else if t == 400 {
            setpoint = -30.0;
        } else if t == 500 {
            setpoint = 100.0;
        }
        let dt = timer.get_dt().unwrap_or(0.0);
        let (control, _) = controller.update((process - setpoint) * 20.0, 0.0, dt);
        process = control * 2.0; // multiply by two because pid.gnuplot was created with the old horizontal/vertical mirror angle definition
        setpoint_history.push(setpoint);
        process_history.push(process);
    }
    assert_gnuplot("pid", &setpoint_history, &process_history);
}

fn assert_gnuplot(name: &str, setpoint_history: &[f64], process_history: &[f64]) {
    let render = |mut acc: String, (t, y)| {
        acc.push_str(&format!("{t} {y:.03}\n"));
        acc
    };
    let setpoint_data = setpoint_history.iter().enumerate().fold(String::new(), render);
    let process_data = process_history.iter().enumerate().fold(String::new(), render);
    let result = format!(
        r#"$setpoint << EOD
{setpoint_data}EOD
$process << EOD
{process_data}EOD
plot "$process" with lines, "$setpoint" with lines
"#
    );
    if fs::read_to_string(format!("tests/eye_pid_controller/{name}.gnuplot")).unwrap_or_default()
        != result
    {
        fs::write(format!("tests/eye_pid_controller/{name}.gnuplot.unmatched"), result.as_bytes())
            .unwrap();
        panic!("tests/eye_pid_controller/{name}.gnuplot doesn't match");
    }
}
