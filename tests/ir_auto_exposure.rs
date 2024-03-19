use orb::{
    agents::ir_auto_exposure::{ExposureController, DEFAULT_EXPOSURE_RANGE},
    pid,
    pid::Timer,
};
use std::fs;

#[test]
fn test_pid() {
    const DT: f64 = 0.03;
    let mut exposure = 0.0;
    let mut timer = pid::ConstDelta::from(DT);
    let mut controller = ExposureController::new(0.0, exposure);
    let mut setpoint_history = Vec::new();
    let mut process_history = Vec::new();
    let setpoint = 135.0;
    let ratio = f64::from(*DEFAULT_EXPOSURE_RANGE.end() - *DEFAULT_EXPOSURE_RANGE.start())
        / f64::from(u8::MAX);
    let mut t = 0.0;
    while t < 2.0 {
        let dt = timer.get_dt().unwrap_or(0.0);
        let (_, new_exposure) = controller.update(
            exposure / ratio,
            setpoint,
            f64::from(*DEFAULT_EXPOSURE_RANGE.start())..=f64::from(*DEFAULT_EXPOSURE_RANGE.end()),
            dt,
        );
        exposure = new_exposure;
        setpoint_history.push(setpoint * ratio);
        process_history.push(exposure);
        t += DT;
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
plot "$setpoint" with lines, "$process" with lines
"#
    );
    if fs::read_to_string(format!("tests/ir_auto_exposure/{name}.gnuplot")).unwrap_or_default()
        != result
    {
        fs::write(format!("tests/ir_auto_exposure/{name}.gnuplot.unmatched"), result.as_bytes())
            .unwrap();
        panic!("tests/ir_auto_exposure/{name}.gnuplot doesn't match");
    }
}
