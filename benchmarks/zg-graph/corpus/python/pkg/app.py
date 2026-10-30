from .calc import add as plus
import pkg.calc as calc


def run():
    return calc.twice(plus(1, 2))


def shadow(plus):
    return plus(2, 3)


def dynamic(obj):
    return obj.execute()
