cmake_minimum_required(VERSION 3.16)
project(slang_playground_renderer C CXX)

# Off by default would be more conservative, but every C/C++ binding this
# repo has today is a sample, so ON matches what most people configuring
# this actually want.
option(BUILD_SAMPLES "Build example programs for the renderer C/C++ bindings" ON)

add_subdirectory(capi)
