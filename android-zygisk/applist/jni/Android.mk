LOCAL_PATH := $(call my-dir)

include $(CLEAR_VARS)
LOCAL_MODULE    := applist
LOCAL_SRC_FILES := main.cpp
LOCAL_LDLIBS    := -llog -lstdc++
include $(BUILD_SHARED_LIBRARY)
