#[cfg(test)]
mod tests {
    use eyre::{Context, Result};
    use pyo3::Python;

    use crate::agents::python::iris::{EstimateOutput, PipelineOutput};
    use ai_interface::PyError;

    const EXAMPLE_IRIS_OUTPUT: &str = r"{
    'error': None,
    'normalized_image': None,
    'normalized_image_resized': None,
    'iris_template': {'iris_codes': 'E0zFXyJgq2+/sTFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFdw==',
                      'mask_codes': '///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////8zMzM/////////////////////////////////////////////////////////////////////////////////////////////////////////8zP/zMzAAAAAAAAAAAAAAAAAAAAAAAzP/////////////////////////////////////////////////////////////////////////////////////////////////////zAAAAAAAAAAAAAAAAAAAAAAAAAAAAAADM//////////////////////////////////////////////////////////////////////////////////////////////////zMAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAzP//////////////////////////////////////////////////////////////////////////////////////////////zMwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAMz////////////////////////////////////////////////////////////////////////////////////////////MzMAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAzM////////////////////////////////////////////////////////////////////////////////////////zMwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAzM///////////////////////////////////////////////////////////////////////////////////////MzAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAzMz////////////////////////////////////////////////////////////////////////////////////MzMAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAADMz//////////////////////////////////////////////////////////////////////////////////8zMwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAADMz/////////////////////////////////////////////////////////////////////////////////zMzMAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAADMzP///////////////////////////////////////////////////////////////////////////////MzMzAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAADMzP//////w==',
                      'iris_code_version': '1.7.2'},
    'metadata': {'eye_centers': {'iris_center': (564.8768229058003,
                                                 398.9210807004433),
                                 'pupil_center': (578.7788155676323,
                                                  401.6670096215058)},
                 'eye_orientation': 0.007373233121502842,
                 'eye_side': 'left',
                 'image_size': (1440, 1080),
                 'iris_bbox': {'x_max': 877.7410888671875,
                               'x_min': 254.19358825683594,
                               'y_max': 705.7726440429688,
                               'y_min': 93.87154388427734},
                 'iris_version': '1.5.1',
                 'occlusion30': 0.9950829028434285,
                 'occlusion90': 0.8284294618143734,
                 'ellipticity': {'pupil_ellipticity': 0.04, 'iris_ellipticity': 0.04},
                 'offgaze_score': 0.18155832771958066,
                 'pupil_to_iris_property': {'pupil_to_iris_center_dist_ratio': 0.04516609061229463,
                                            'pupil_to_iris_diameter_ratio': 0.388149379214243},
                 'template_property': {'visible_ratio': 0.78390625, 'lower_visible_ratio': 1.0, 'upper_visible_ratio': 0.5678125, 'abnormal_mask_ratio': 0.0215625, 'weighted_abnormal_mask_ratio': 0.021731390308341404, 'maskcode_hist': None}}
    }";

    const EXAMPLE_IRIS_OUTPUT_WITH_ERROR: &str = r#"{
    'error': {'error_type': 'VectorizationError',
              'message': 'Geometry raster verification failed.',
              'traceback': '  File '
                 '"/home/worldcoin/venv/lib/python3.8/site-packages/iris/pipelines/iris_pipeline.py", '
                 'line 104, in run\n'
                 '    _ = self.nodes[node.name](**input_kwargs)\n'
                 '  File '
                 '"/home/worldcoin/venv/lib/python3.8/site-packages/iris/io/class_configs.py", '
                 'line 58, in __call__\n'
                 '    return self.execute(*args, **kwargs)\n'
                 '  File '
                 '"/home/worldcoin/venv/lib/python3.8/site-packages/iris/io/class_configs.py", '
                 'line 69, in execute\n'
                 '    result = self.run(*args, **kwargs)\n'
                 '  File '
                 '"/home/worldcoin/venv/lib/python3.8/site-packages/iris/nodes/vectorization/contouring.py", '
                 'line 73, in run\n'
                 '    raise VectorizationError("Geometry raster '
                 'verification failed.")\n'},
    'iris_template': None,
    'normalized_image': None,
    'normalized_image_resized': None,
    'metadata': {'eye_centers': None,
       'eye_orientation': None,
       'eye_side': 'left',
       'image_size': (1440, 1080),
       'iris_bbox': None,
       'iris_version': '1.7.2',
       'occlusion30': None,
       'occlusion90': None,
       'ellipticity': None,
       'offgaze_score': None,
       'pupil_to_iris_property': None,
       'template_property': None}}"#;

    #[test]
    fn test_extract_normal_output() -> Result<()> {
        Python::with_gil(|py| {
            let output: EstimateOutput = py
                .eval(EXAMPLE_IRIS_OUTPUT, None, None)
                .wrap_err("eval failed")?
                .extract::<PipelineOutput>()?
                .try_into()?;
            assert!(output.iris_code.starts_with("E0zFXyJgq2+/sT"));
            assert!(output.mask_code.ends_with("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAADMzP//////w=="));
            assert_eq!(output.metadata.image_size, Some((1440, 1080)));
            Ok(())
        })
    }

    #[test]
    fn test_extract_output_with_errors() -> Result<()> {
        Python::with_gil(|py| {
            let output: Result<EstimateOutput, PyError> = py
                .eval(EXAMPLE_IRIS_OUTPUT_WITH_ERROR, None, None)
                .wrap_err("eval failed")?
                .extract::<PipelineOutput>()?
                .try_into();
            match output {
                Ok(_) => panic!("Output should be an Err"),
                Err(e) => {
                    assert_eq!(e.error_type, "VectorizationError");
                    assert_eq!(e.message, "Geometry raster verification failed.");
                }
            }
            Ok(())
        })
    }
}
