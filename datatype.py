from enum import Enum, unique
from pydantic import BaseModel

class Item(BaseModel, extra="forbid"):
    name: str
    """物品名"""
    price: int
    """交易行价格"""
    enter_price: int
    """战备"""

"""
关于
"""

class AttachmentProperty(BaseModel, extra="forbid"):
    # 后坐力控制
    # 属性均为倍率, 乘算
    full_auto_vertical_recoil: float
    """全自动垂直后坐力倍率"""
    full_auto_horizontal_recoil: float
    """全自动水平后坐力倍率"""
    single_fire_vertical_recoil: float
    """单点垂直后坐力倍率"""
    single_fire_horizontal_recoil: float
    """单点水平后坐力倍率"""
    firing_stability: float
    """开火稳定性"""

    """操控"""


class Attachment(Item, extra="forbid"):
    name: str
    """物品名"""
    price: int
    """交易行价格"""
    enter_price: int
    """战备"""
    type: str
    """分类"""
    property: 

class Weapon(Item, extra="forbid"):
    slot_list: dict[str, list[Attachment]]