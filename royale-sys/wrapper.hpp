#include <cstddef>
#include <inttypes.h>

typedef struct Camera Camera;

typedef struct Frame Frame;

typedef struct StringVector StringVector;

class DataListener;

typedef void (*DataListenerCallback)(const Frame* frame, void* payload);

int
camera_attach(DataListenerCallback callback,
              void* payload,
              Camera** camera,
              DataListener** listener);

int
camera_get_use_cases(Camera* camera, StringVector* useCases);

int
camera_set_use_case(Camera* camera, const char* useCase);

int
camera_get_max_frame_rate(Camera* camera, uint16_t* framerate);

int
camera_get_frame_rate(Camera* camera, uint16_t* framerate);

int
camera_set_frame_rate(Camera* camera, uint16_t framerate);

int
camera_get_exposure_mode(Camera* camera, bool* isManual);

int
camera_set_exposure_mode(Camera* camera, bool isManual);

int
camera_get_exposure_limits(Camera* camera, uint32_t* low, uint32_t* high);

int
camera_set_exposure_time(Camera* camera, uint32_t exposureTime);

int
camera_capture_start(Camera* camera);

int
camera_capture_stop(Camera* camera);

void
camera_delete(Camera* camera, DataListener* listener);

void
frame_metadata(const Frame* frame,
               uint16_t* width,
               uint16_t* height,
               uint64_t* timestamp);

void
frame_point(const Frame* frame,
            size_t index,
            float* x,
            float* y,
            float* z,
            float* noise,
            uint16_t* grayValue,
            uint8_t* depthConfidence);

bool
is_camera_status_success(int camera_status);

char*
camera_status_to_string(int camera_status);

void
delete_string(char* string);

StringVector*
new_string_vector();

int
string_vector_length(StringVector* vector);

char*
string_vector_get(StringVector* vector, int index);

void
delete_string_vector(StringVector* vector);
