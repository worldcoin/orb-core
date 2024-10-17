#include "wrapper.hpp"
#include <royale.hpp>

#define TRY(METHOD_TO_INVOKE)                                                  \
    do {                                                                       \
        auto status = METHOD_TO_INVOKE;                                        \
        if (status != royale::CameraStatus::SUCCESS) {                         \
            return static_cast<int>(status);                                   \
        }                                                                      \
    } while (0)

#define OK static_cast<int>(royale::CameraStatus::SUCCESS)

class DataListener : public royale::IDepthDataListener
{
  public:
    DataListener(DataListenerCallback callback, void* payload)
      : callback(callback)
      , payload(payload)
    {
    }

    void onNewData(const royale::DepthData* data) override
    {
        callback(reinterpret_cast<const Frame*>(data), payload);
    }

  private:
    DataListenerCallback callback;

    void* payload;
};

inline royale::ICameraDevice*
toCameraDevice(Camera* camera)
{
    return reinterpret_cast<royale::ICameraDevice*>(camera);
}

int
camera_attach(DataListenerCallback callback,
              void* payload,
              Camera** camera,
              DataListener** listener)
{
    royale::CameraManager manager;
    auto connectedCameraList = manager.getConnectedCameraList();
    if (connectedCameraList.count() > 0) {
        royale::ICameraDevice* cameraDevice =
          manager.createCamera(connectedCameraList[0]).release();
        *camera = reinterpret_cast<Camera*>(cameraDevice);
        if (cameraDevice != nullptr) {
            TRY(cameraDevice->initialize());
            *listener = new DataListener(callback, payload);
            TRY(cameraDevice->registerDataListener(*listener));
        }
    }
    return OK;
}

int
camera_get_use_cases(Camera* camera, StringVector* useCases)
{
    TRY(toCameraDevice(camera)->getUseCases(
      *reinterpret_cast<royale::Vector<royale::String>*>(useCases)));
    return OK;
}

int
camera_set_use_case(Camera* camera, const char* useCase)
{
    royale::String string = royale::String(useCase);
    TRY(toCameraDevice(camera)->setUseCase(string));
    return OK;
}

int
camera_get_max_frame_rate(Camera* camera, uint16_t* framerate)
{
    TRY(toCameraDevice(camera)->getMaxFrameRate(*framerate));
    return OK;
}

int
camera_get_frame_rate(Camera* camera, uint16_t* framerate)
{
    TRY(toCameraDevice(camera)->getFrameRate(*framerate));
    return OK;
}

int
camera_set_frame_rate(Camera* camera, uint16_t framerate)
{
    TRY(toCameraDevice(camera)->setFrameRate(framerate));
    return OK;
}

int
camera_get_exposure_mode(Camera* camera, bool* isManual)
{
    royale::ExposureMode exposureMode;
    TRY(toCameraDevice(camera)->getExposureMode(exposureMode));
    *isManual = exposureMode == royale::ExposureMode::MANUAL;
    return OK;
}

int
camera_set_exposure_mode(Camera* camera, bool isManual)
{
    royale::ExposureMode exposureMode =
      isManual ? royale::ExposureMode::MANUAL : royale::ExposureMode::AUTOMATIC;
    TRY(toCameraDevice(camera)->setExposureMode(exposureMode));
    return OK;
}

int
camera_get_exposure_limits(Camera* camera, uint32_t* low, uint32_t* high)
{
    auto exposureLimits = royale::Pair<uint32_t, uint32_t>();
    TRY(toCameraDevice(camera)->getExposureLimits(exposureLimits));
    *low = exposureLimits.first;
    *high = exposureLimits.second;
    return OK;
}

int
camera_set_exposure_time(Camera* camera, uint32_t exposureTime)
{
    TRY(toCameraDevice(camera)->setExposureTime(exposureTime));
    return OK;
}

int
camera_capture_start(Camera* camera)
{
    TRY(toCameraDevice(camera)->startCapture());
    return OK;
}

int
camera_capture_stop(Camera* camera)
{
    TRY(toCameraDevice(camera)->stopCapture());
    return OK;
}

void
camera_delete(Camera* camera, DataListener* listener)
{
    if (listener != nullptr) {
        toCameraDevice(camera)->unregisterDataListener();
        delete listener;
    }
    delete toCameraDevice(camera);
}

inline const royale::DepthData*
toDepthData(const Frame* frame)
{
    return reinterpret_cast<const royale::DepthData*>(frame);
}

void
frame_metadata(const Frame* frame,
               uint16_t* width,
               uint16_t* height,
               uint64_t* timestamp)
{
    const royale::DepthData* data = toDepthData(frame);
    *width = data->width;
    *height = data->height;
    *timestamp = data->timeStamp.count();
}

void
frame_point(const Frame* frame,
            size_t index,
            float* x,
            float* y,
            float* z,
            float* noise,
            uint16_t* grayValue,
            uint8_t* depthConfidence)
{
    royale::DepthPoint point = toDepthData(frame)->points[index];
    *x = point.x;
    *y = point.y;
    *z = point.z;
    *noise = point.noise;
    *grayValue = point.grayValue;
    *depthConfidence = point.depthConfidence;
}

inline royale::CameraStatus
toCameraStatus(int camera_status)
{
    return static_cast<royale::CameraStatus>(camera_status);
}

bool
is_camera_status_success(int camera_status)
{
    return toCameraStatus(camera_status) == royale::CameraStatus::SUCCESS;
}

char*
camera_status_to_string(int camera_status)
{
    royale::String string =
      royale::getStatusString(toCameraStatus(camera_status));
    char* output = new char[string.size()];
    strcpy(output, string.data());
    return output;
}

void
delete_string(char* string)
{
    delete string;
}

StringVector*
new_string_vector()
{
    auto vector = new royale::Vector<royale::String>();
    return reinterpret_cast<StringVector*>(vector);
}

int
string_vector_length(StringVector* vector)
{
    return reinterpret_cast<royale::Vector<royale::String>*>(vector)->count();
}

char*
string_vector_get(StringVector* vector, int index)
{
    royale::String string =
      (*reinterpret_cast<royale::Vector<royale::String>*>(vector))[index];
    char* output = new char[string.size()];
    strcpy(output, string.data());
    return output;
}

void
delete_string_vector(StringVector* vector)
{
    delete reinterpret_cast<royale::Vector<royale::String>*>(vector);
}
