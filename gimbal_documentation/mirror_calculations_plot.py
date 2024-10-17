# import transforms3d as t3d
import numpy as np
import math
from numpy.linalg import norm
import matplotlib.pyplot as plt


def angle_of_vect(v1, v2):
    return math.acos(v1.dot(v2) / (norm(v1) * norm(v2)))


def rad2deg(a):
    return a / math.pi * 180


def angle_of_vect_deg(v1, v2):
    return rad2deg(angle_of_vect(v1, v2))


def mirror_vect(v, n):
    return v - 2 * v.dot(n) * n


in_vec = np.array([0, -1, 0])
x_axis = np.array([1, 0, 0])  # looks towards target
z_axis = np.array([0, 0, 1])  # looks into sky


def calc_viewing_angles(mirror_theta_deg: float, mirror_phi_deg: float) -> (float, float):
    # theta: vertical direction of mirror
    # 0° -> mirror looks into sky
    # 90° -> mirror looks to horizon
    # phi: horizontal direction of mirror
    # 0° -> mirror looks towards target (x axis)

    mirror_theta_rad = mirror_theta_deg / 180 * math.pi
    mirror_phi_rad = mirror_phi_deg / 180 * math.pi
    mirror_norm = np.array(
        [math.sin(mirror_theta_rad) * math.cos(mirror_phi_rad),
         math.sin(mirror_theta_rad) * math.sin(mirror_phi_rad),
         math.cos(mirror_theta_rad)]
    )
    out_vec = mirror_vect(in_vec, mirror_norm)

    out_angle_theta_deg = math.acos(out_vec[2]) / math.pi * 180
    out_angle_phi_deg = math.atan(out_vec[1] / out_vec[0]) / math.pi * 180

    return out_angle_theta_deg, out_angle_phi_deg


def calc_viewing_angles_simplified(mirror_theta_deg: float, mirror_phi_deg: float) -> (float, float):
    # theta: vertical direction of mirror
    # 0° -> mirror looks into sky
    # 90° -> mirror looks to horizon
    # phi: horizontal direction of mirror
    # 0° -> mirror looks towards target (x axis)

    mirror_theta_rad = mirror_theta_deg / 180 * math.pi
    mirror_phi_rad = mirror_phi_deg / 180 * math.pi

    out_angle_theta_rad = math.acos(math.sin(2 * mirror_theta_rad) * math.sin(mirror_phi_rad))
    #    out_angle_phi_rad = math.atan((-1 + 1 / 2 * (1 - math.cos(2*mirror_theta_rad))*(1 - math.cos(2*mirror_phi_rad)))/(1/2*(1-math.cos(2*mirror_theta_rad))*math.sin(2*mirror_phi_rad)))
    out_angle_phi_rad = math.atan(
        math.tan(mirror_phi_rad) - 1 / (0.5 * (1 - math.cos(2 * mirror_theta_rad)) * math.sin(2 * mirror_phi_rad)))

    out_angle_theta_deg = out_angle_theta_rad / math.pi * 180
    out_angle_phi_deg = out_angle_phi_rad / math.pi * 180

    return out_angle_theta_deg, out_angle_phi_deg


def calc_mirror_angles(viewing_angle_theta_deg: float, viewing_angle_phi_deg: float) -> (float, float):
    viewing_angle_theta_rad = viewing_angle_theta_deg / 180 * math.pi
    viewing_angle_phi_rad = viewing_angle_phi_deg / 180 * math.pi

    e_1 = math.sin(viewing_angle_theta_rad) * math.cos(viewing_angle_phi_rad)
    e_2 = math.sin(viewing_angle_theta_rad) * math.sin(viewing_angle_phi_rad)
    e_3 = math.cos(viewing_angle_theta_rad)

    n_scaling = 1 / math.sqrt(e_1 ** 2 + ((1 + e_2) ** 2) + e_3 ** 2)
    n_1 = e_1 * n_scaling
    n_2 = (1 + e_2) * n_scaling
    n_3 = e_3 * n_scaling

    mirror_theta_rad = math.acos(n_3)
    mirror_phi_rad = math.atan(n_2 / n_1)

    mirror_theta_deg = mirror_theta_rad / math.pi * 180
    mirror_phi_deg = mirror_phi_rad / math.pi * 180

    return mirror_theta_deg, mirror_phi_deg


def calc_mirror_angles_simplified(viewing_angle_theta_deg: float, viewing_angle_phi_deg: float) -> (float, float):
    viewing_angle_theta_rad = viewing_angle_theta_deg / 180 * math.pi
    viewing_angle_phi_rad = viewing_angle_phi_deg / 180 * math.pi

    e_1 = math.sin(viewing_angle_theta_rad) * math.cos(viewing_angle_phi_rad)
    e_2 = math.sin(viewing_angle_theta_rad) * math.sin(viewing_angle_phi_rad)
    e_3 = math.cos(viewing_angle_theta_rad)

    mirror_theta_rad = math.acos(e_3 * 1 / math.sqrt(e_1 ** 2 + ((1 + e_2) ** 2) + e_3 ** 2))
    mirror_phi_rad = math.atan((1 + e_2) / e_1)

    mirror_theta_deg = mirror_theta_rad / math.pi * 180
    mirror_phi_deg = mirror_phi_rad / math.pi * 180

    return mirror_theta_deg, mirror_phi_deg


for theta_m in range(60, 121, 1):
    for phi_m in range(30, 61, 5):
        point = calc_viewing_angles(theta_m, phi_m)
        plt.plot(point[1], point[0], marker='.', color="green")
        point_alt = calc_viewing_angles_simplified(theta_m, phi_m)
        plt.plot(point_alt[1], point_alt[0], marker='x', color="red")
        point_mirror = calc_mirror_angles(point[0], point[1])
        plt.plot(point_mirror[1], point_mirror[0], marker='o', color="blue")
        point_mirror_alt = calc_mirror_angles_simplified(point[0], point[1])
        plt.plot(point_mirror_alt[1], point_mirror_alt[0], marker='.', color="red")
plt.show()
