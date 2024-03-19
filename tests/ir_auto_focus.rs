use orb::{
    agents::ir_auto_focus::{DerivedSignal, PID_INTEGRAL, PID_PROPORTIONAL},
    pid::{ConstDelta, Pid, Timer},
};
use std::fs;

#[test]
fn test_derived_signal_1() {
    test_derived_signal(1);
}

#[test]
fn test_derived_signal_2() {
    test_derived_signal(2);
}

#[test]
fn test_derived_signal_3() {
    test_derived_signal(3);
}

#[test]
fn test_derived_signal_4() {
    test_derived_signal(4);
}

#[test]
fn test_pid() {
    const DT: f64 = 0.05;
    let mut timer = ConstDelta::from(DT);
    let mut pid = Pid::default().with_proportional(PID_PROPORTIONAL).with_integral(PID_INTEGRAL);
    let mut setpoint_history = Vec::new();
    let mut process_history = Vec::new();
    let mut process_sharpness = 0.8550;
    let mut process_focus = -37.0;
    let setpoint_sharpness = 1.8509;
    let setpoint_focus = -29.0;
    let ratio = (setpoint_sharpness - process_sharpness) / (setpoint_focus - process_focus);
    let mut t = 0.0;
    while t < 0.5 {
        let dt = timer.get_dt().unwrap_or(0.0);
        let control = pid.advance(setpoint_sharpness, process_sharpness, dt);
        process_focus += control;
        process_sharpness += control * ratio;
        setpoint_history.push(setpoint_sharpness);
        process_history.push(process_sharpness);
        t += DT;
    }
    assert_pid_gnuplot("pid", &setpoint_history, &process_history);
}

fn test_derived_signal(sample_n: usize) {
    let mut derived = DerivedSignal::default();
    let mut sharpness_history = Vec::new();
    let mut derived_history = Vec::new();
    let mut last_focus = None;
    for line in
        fs::read_to_string(format!("tests/ir_auto_focus/sample_{sample_n}")).unwrap().lines()
    {
        let row = line.splitn(2, ' ').map(str::parse).collect::<Result<Vec<_>, _>>().unwrap();
        let focus = row[0];
        let sharpness = row[1];
        if sharpness == 0.0 {
            continue;
        }
        sharpness_history.push((focus, sharpness));
        let dt = last_focus.map_or(0.0, |last_focus| (focus - last_focus) * 3.0);
        if let Some(derived) = derived.add(sharpness, dt) {
            derived_history.push((focus, derived));
        }
        last_focus = Some(focus);
    }
    assert_derived_signal_gnuplot(
        &format!("derived_{sample_n}"),
        &sharpness_history,
        &derived_history,
    );
}

fn assert_derived_signal_gnuplot(
    name: &str,
    sharpness_history: &[(f64, f64)],
    derived_history: &[(f64, f64)],
) {
    let render = |mut acc: String, &(x, y)| {
        acc.push_str(&format!("{x:.04} {y:.04}\n"));
        acc
    };
    let sharpness_data = sharpness_history.iter().fold(String::new(), render);
    let derived_data = derived_history.iter().fold(String::new(), render);
    let result = format!(
        r#"$sharpness << EOD
{sharpness_data}EOD
$derived << EOD
{derived_data}EOD
set y2tics
set ytics nomirror
plot [0:1] 0 axes x1y2, "$sharpness" with lines, "$derived" with lines axes x1y2
"#
    );
    if fs::read_to_string(format!("tests/ir_auto_focus/{name}.gnuplot")).unwrap_or_default()
        != result
    {
        fs::write(format!("tests/ir_auto_focus/{name}.gnuplot.unmatched"), result.as_bytes())
            .unwrap();
        panic!("tests/ir_auto_focus/{name}.gnuplot doesn't match");
    }
}

fn assert_pid_gnuplot(name: &str, setpoint_history: &[f64], process_history: &[f64]) {
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
    if fs::read_to_string(format!("tests/ir_auto_focus/{name}.gnuplot")).unwrap_or_default()
        != result
    {
        fs::write(format!("tests/ir_auto_focus/{name}.gnuplot.unmatched"), result.as_bytes())
            .unwrap();
        panic!("tests/ir_auto_focus/{name}.gnuplot doesn't match");
    }
}
